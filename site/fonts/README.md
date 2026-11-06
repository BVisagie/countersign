# Fonts

The page serves its own fonts, so a visit makes no third-party request. Both
are under the [SIL Open Font License 1.1](OFL-Archivo.txt), and have no
reserved font name, so these reduced copies keep their names.

| File | Source | Kept |
| --- | --- | --- |
| `archivo.woff2` | `ofl/archivo/Archivo[wdth,wght].ttf` | weights 400–800, widths 88–100% |
| `jetbrains-mono.woff2` | `ofl/jetbrainsmono/JetBrainsMono[wght].ttf` | weights 400–700 |

The sources come from [google/fonts](https://github.com/google/fonts) at
commit `38411257417e616c5d4daa1b9e91a3dbcebe1c2e`. To remake them, with
`fonttools` and `brotli` installed:

```bash
latin='U+0020-007E,U+00A0-00FF,U+2013,U+2014,U+2018,U+2019,U+201C,U+201D,U+2022,U+2026'
fonttools varLib.instancer 'Archivo[wdth,wght].ttf' wght=400:800 wdth=88:100 -o archivo.ttf
fonttools varLib.instancer 'JetBrainsMono[wght].ttf' wght=400:700 -o jetbrains-mono.ttf
pyftsubset archivo.ttf --unicodes="$latin" --layout-features=kern,ccmp,locl,mark,mkmk,tnum \
  --flavor=woff2 --output-file=archivo.woff2
pyftsubset jetbrains-mono.ttf --unicodes="$latin,U+21E7,U+25CF" --layout-features=ccmp,locl,mark,mkmk \
  --flavor=woff2 --output-file=jetbrains-mono.woff2
```

Neither font has `↵`, and Archivo has no `↗`, so the page draws those as icons.
