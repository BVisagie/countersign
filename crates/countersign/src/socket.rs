//! The app's connection to `countersignd`.
//!
//! Runs on the GTK main loop with GIO's asynchronous socket API: one task
//! reads lines and resolves pending requests or forwards events, and one
//! task writes queued requests in order.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use countersign_ipc::{ErrorCode, Event, IpcError, Request, RequestFrame, Response, ServerFrame};
use gtk::gio;
use gtk::gio::prelude::*;
use gtk::glib;
use zeroize::Zeroizing;

type Reply = async_channel::Sender<Result<Response, IpcError>>;

pub struct SocketClient {
    writer: async_channel::Sender<Zeroizing<Vec<u8>>>,
    pending: Rc<RefCell<HashMap<u64, Reply>>>,
    next_id: Cell<u64>,
    alive: Rc<Cell<bool>>,
}

impl SocketClient {
    /// Connects to the daemon, starting it first if it is not running.
    pub async fn connect(events: async_channel::Sender<Option<Event>>) -> Result<Rc<Self>, IpcError> {
        let path = countersign_ipc::socket_path();
        let connection = match try_connect(&path).await {
            Ok(c) => c,
            Err(_) => {
                start_daemon()?;
                let mut last = None;
                let mut connected = None;
                for _ in 0..40 {
                    glib::timeout_future(Duration::from_millis(75)).await;
                    match try_connect(&path).await {
                        Ok(c) => {
                            connected = Some(c);
                            break;
                        }
                        Err(e) => last = Some(e),
                    }
                }
                connected.ok_or_else(|| {
                    IpcError::new(
                        ErrorCode::Internal,
                        format!(
                            "Countersign's background service did not start: {}",
                            last.map(|e| e.to_string()).unwrap_or_default()
                        ),
                    )
                })?
            }
        };

        let (writer, queue) = async_channel::unbounded::<Zeroizing<Vec<u8>>>();
        let pending: Rc<RefCell<HashMap<u64, Reply>>> = Rc::default();
        let alive = Rc::new(Cell::new(true));
        let output = connection.output_stream();
        glib::spawn_future_local(async move {
            while let Ok(line) = queue.recv().await {
                let buf: Vec<u8> = line.to_vec();
                if output.write_all_future(buf, glib::Priority::DEFAULT).await.is_err() {
                    break;
                }
            }
        });

        let input = gio::DataInputStream::new(&connection.input_stream());
        let reader_pending = pending.clone();
        let reader_alive = alive.clone();
        glib::spawn_future_local(async move {
            // Keep the connection alive as long as the reader runs.
            let _connection = connection;
            loop {
                let line = match input.read_line_utf8_future(glib::Priority::DEFAULT).await {
                    Ok(Some(line)) => line,
                    _ => break,
                };
                let Ok(frame) = serde_json::from_str::<ServerFrame>(&line) else { continue };
                match frame {
                    ServerFrame::Response { id, result } => {
                        if let Some(reply) = reader_pending.borrow_mut().remove(&id) {
                            let _ = reply.try_send(result);
                        }
                    }
                    ServerFrame::Event { event } => {
                        let _ = events.send(Some(event)).await;
                    }
                }
            }
            reader_alive.set(false);
            for (_, reply) in reader_pending.borrow_mut().drain() {
                let _ = reply.try_send(Err(disconnected()));
            }
            let _ = events.send(None).await;
        });

        Ok(Rc::new(SocketClient { writer, pending, next_id: Cell::new(1), alive }))
    }

    pub fn is_alive(&self) -> bool {
        self.alive.get()
    }

    pub async fn request(&self, request: Request) -> Result<Response, IpcError> {
        if !self.alive.get() {
            return Err(disconnected());
        }
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let (reply, response) = async_channel::bounded(1);
        self.pending.borrow_mut().insert(id, reply);
        let mut line = Zeroizing::new(
            serde_json::to_vec(&RequestFrame { id, request })
                .map_err(|e| IpcError::new(ErrorCode::Internal, e.to_string()))?,
        );
        line.push(b'\n');
        if self.writer.send(line).await.is_err() {
            self.pending.borrow_mut().remove(&id);
            return Err(disconnected());
        }
        response.recv().await.unwrap_or_else(|_| Err(disconnected()))
    }
}

fn disconnected() -> IpcError {
    IpcError::new(ErrorCode::Internal, "Lost the connection to Countersign's background service.")
}

async fn try_connect(path: &std::path::Path) -> Result<gio::SocketConnection, glib::Error> {
    let address = gio::UnixSocketAddress::new(path);
    gio::SocketClient::new().connect_future(&address).await
}

/// Starts `countersignd` from next to this executable, or from `PATH`.
fn start_daemon() -> Result<(), IpcError> {
    let beside = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|d| d.join("countersignd")));
    let program = beside.filter(|p| p.is_file()).unwrap_or_else(|| PathBuf::from("countersignd"));
    std::process::Command::new(&program)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .spawn()
        .map(drop)
        .map_err(|e| IpcError::new(ErrorCode::Internal, format!("Cannot start {}: {e}", program.display())))
}
