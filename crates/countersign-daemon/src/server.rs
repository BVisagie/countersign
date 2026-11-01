//! The private socket the app connects to.
//!
//! Only processes of the same user may connect. Each line is one JSON
//! request; requests run concurrently, so a status check never waits behind
//! an unlock. Responses and events share the connection.

use std::sync::Arc;

use countersign_ipc::{ErrorCode, IpcError, MAX_FRAME, RequestFrame, ServerFrame};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc};
use zeroize::{Zeroize, Zeroizing};

use crate::core::Daemon;

pub async fn serve(daemon: Arc<Daemon>, listener: UnixListener) {
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    loop {
        let stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            Err(e) => {
                tracing::warn!("accepting a connection failed: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            }
        };
        match stream.peer_cred() {
            Ok(cred) if cred.uid() == uid => {}
            _ => {
                tracing::warn!("refused a connection from another user");
                continue;
            }
        }
        tokio::spawn(connection(daemon.clone(), stream));
    }
}

async fn connection(daemon: Arc<Daemon>, stream: UnixStream) {
    daemon.client_connected();
    let (read, mut write) = stream.into_split();
    let (out, mut outgoing) = mpsc::channel::<Zeroizing<Vec<u8>>>(64);

    let writer = tokio::spawn(async move {
        while let Some(line) = outgoing.recv().await {
            if write.write_all(&line).await.is_err() {
                break;
            }
        }
    });

    let mut events = daemon.subscribe_events();
    let event_out = out.clone();
    let forwarder = tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => {
                    if event_out.send(encode(&ServerFrame::Event { event })).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let mut reader = BufReader::new(read);
    loop {
        let mut line = Zeroizing::new(Vec::new());
        match (&mut reader).take(MAX_FRAME as u64 + 1).read_until(b'\n', &mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) if line.len() > MAX_FRAME => break,
            Ok(_) => {}
        }
        let frame = match serde_json::from_slice::<RequestFrame>(&line) {
            Ok(frame) => frame,
            Err(_) => {
                let reply = ServerFrame::Response {
                    id: 0,
                    result: Err(IpcError::new(ErrorCode::Invalid, "Malformed request.")),
                };
                let _ = out.send(encode(&reply)).await;
                break;
            }
        };
        line.zeroize();
        let daemon = daemon.clone();
        let out = out.clone();
        tokio::spawn(async move {
            let result = daemon.handle(frame.request).await;
            let _ = out.send(encode(&ServerFrame::Response { id: frame.id, result })).await;
        });
    }
    forwarder.abort();
    drop(out);
    let _ = writer.await;
    daemon.client_disconnected();
}

fn encode(frame: &ServerFrame) -> Zeroizing<Vec<u8>> {
    let mut line = Zeroizing::new(serde_json::to_vec(frame).unwrap_or_default());
    line.push(b'\n');
    line
}
