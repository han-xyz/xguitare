# xguitare

Low-latency guitar amp and effects rack for Linux, headless first.
Rust, JACK API (runs natively on PipeWire via pipewire-jack), own DSP, TOML presets.

One guitar in, stereo out. A Daemon runs the audio; the `xguitare` CLI talks to it over a Unix socket. A GUI or script can use the same socket: see [docs/control-api.md](docs/control-api.md).

## Requirements

- Linux with PipeWire and `pipewire-jack` (the JACK client library PipeWire provides). xguitare is a JACK client and never starts a JACK server itself.
- A Rust toolchain with edition 2024 support (Rust 1.85 or newer), e.g. from [rustup](https://rustup.rs).

## Install

```sh
cargo install --path crates/xguitare
```

This builds a release binary into `~/.cargo/bin/xguitare`; that directory must be on your `PATH`.

## Run

```sh
xguitare run
```

The Daemon logs to stderr and runs until Ctrl-C (SIGINT) or SIGTERM. The first start with the config shown under [Config](#config) logs (your home directory in place of `/home/you`):

```text
2026-11-03T12:52:12.034094Z  INFO xguitare::daemon: created starter bank in /home/you/.config/xguitare/bank
2026-11-03T12:52:12.034221Z  INFO jack_sys: Loading jack from libjack.so.0.
2026-11-03T12:52:12.036502Z  INFO xguitare::daemon: loaded 0 Clean
2026-11-03T12:52:12.040213Z  INFO xguitare::jack_adapter: jack client active: sample_rate=48000 buffer_size=128
2026-11-03T12:52:12.040576Z  INFO xguitare::jack_adapter: connected Scarlett Solo 4th Gen Input 1 Inst/Line:capture_MONO -> xguitare:in
2026-11-03T12:52:12.040808Z  INFO xguitare::jack_adapter: connected xguitare:out_l -> Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FL
2026-11-03T12:52:12.040993Z  INFO xguitare::jack_adapter: connected xguitare:out_r -> Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FR
```

- `created starter bank in ...` appears on the first run only: the Bank directory did not exist, so the Daemon created it with five starter Presets (Clean, Crunch, Lead, High Gain, Bass) and the cabinet IR `default-cab.wav`. An existing Bank directory is never touched.
- `loaded 0 Clean`: the Daemon starts on the Bank's first Preset (program 0).
- `jack client active`: the sample rate and the buffer size the audio server actually granted.
- `connected a -> b`: one line per port connection made from the `[ports]` config. Without a `[ports]` section and with more than one sound card, the Daemon connects nothing and warns with the candidate port names (`several capture devices; pick one in config ports.input: ...`).

Set `RUST_LOG=debug` (or `warn`, ...) to change the log level; the default is `info`.

## Presets

A Preset is one TOML file in the Bank directory, `~/.config/xguitare/bank/` by default. The starter Preset `02-crunch.toml`:

```toml
name = "Crunch"

[[module]]
type = "gate"
[module.params]
threshold = -65

[[module]]
type = "drive"
[module.params]
drive = 35
tone = 55
level = 0

[[module]]
type = "preamp"
[module.params]
gain = 45
master = -12
stages = 3

[[module]]
type = "tonestack"
[module.params]
bass = 50
middle = 45
treble = 55

[[module]]
type = "cabinet"
ir = "default-cab.wav"

[[module]]
type = "delay"
[module.params]
time = 320
feedback = 25
mix = 12

[[module]]
type = "reverb"
[module.params]
size = 35
mix = 10
```

- Each `[[module]]` is one Module of the Rack, in signal order. `type` is required. `id` is optional and defaults to the type; give one when a Preset has two Modules of the same type (ids must be unique). The CLI addresses Parameters as `<id>.<param>`.
- Omitted Parameters take their defaults; values outside a Parameter's range are clamped.
- `cabinet` needs `ir`: a WAV file (16/24/32-bit PCM or 32-bit float, mono or stereo) relative to the Bank directory. The path must stay inside the Bank directory: no absolute path, no `..` or `.`, so `irs/cab.wav` works and `../cab.wav` is an error. The cabinet turns the mono signal into stereo, so a Rack has at most one cabinet, `delay` and `reverb` come after it and every other Module before it.

Modules and Parameters (`min..max default unit`):

| Module | Parameters |
| --- | --- |
| `gate` | `threshold` −90..0 −60 dB, `attack` 0.1..50 1 ms, `release` 5..1000 100 ms, `hysteresis` 0..20 6 dB |
| `drive` | `drive` 0..100 50 %, `tone` 0..100 50 %, `level` −30..10 0 dB |
| `preamp` | `gain` 0..100 40 %, `bias` −1..1 0, `master` −60..0 −12 dB, `stages` 1..4 3 |
| `tonestack` | `bass`, `middle`, `treble` 0..100 50 % |
| `cabinet` | `level` −20..20 0 dB |
| `delay` | `time` 10..2000 400 ms, `feedback` 0..95 35 %, `mix` 0..100 25 %, `pingpong` 0..1 0 |
| `reverb` | `size` 0..100 50 %, `damping` 0..100 50 %, `mix` 0..100 20 %, `width` 0..100 100 % |
| `gain` | `level` −60..20 0 dB |

Bank numbering: every `*.toml` file in the Bank directory, sorted by file name, numbered from 0. That number is the program number for `load` and for MIDI; prefix file names with `01-`, `02-`, ... to fix the order. A file that does not parse keeps its number and shows in `list` with `!` and the error.

Hot reload: the Daemon watches the Bank directory. Saving the active Preset's file in an editor switches to the new version with a short crossfade; if the file no longer parses, the Daemon logs the error and keeps playing the old version. Adding, removing or renaming files renumbers the Bank.

## CLI

Every subcommand except `run` sends one request to the running Daemon and exits 0 on success; on an error it prints one `error: ...` line to stderr and exits 1. `xguitare --help` lists them.

`status` shows the active Preset, audio settings and port connections:

```console
$ xguitare status
preset: Clean
program: 0
sample_rate: 48000
buffer_size: 128
xruns: 0
callback_max_us: 92
callback_last_us: 40
connections:
  Scarlett Solo 4th Gen Input 1 Inst/Line:capture_MONO -> xguitare:in
  xguitare:out_l -> Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FL
  xguitare:out_r -> Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FR
```

`list` shows the Bank; `*` marks the active Preset:

```console
$ xguitare list
 0* Clean
 1  Crunch
 2  Lead
 3  High Gain
 4  Bass
```

`load` switches Preset by name or program number:

```console
$ xguitare load Crunch
loaded 1 Crunch
$ xguitare load 2
loaded 2 Lead
```

`set` changes one Parameter of the running Preset and prints the value applied:

```console
$ xguitare set preamp.gain 60
preamp.gain = 60
$ xguitare set preamp.gain 150
preamp.gain = 100 (clamped)
```

`get` shows Parameters: all of them, one Module's, or one:

```console
$ xguitare get preamp
preamp.gain = 100 %
preamp.bias = 0.2
preamp.master = -14 dB
preamp.stages = 3
$ xguitare get preamp.gain
preamp.gain = 100 %
```

`set` changes only the running sound. `save` writes the live Parameter values back to the active Preset's file:

```console
$ xguitare save
saved /home/you/.config/xguitare/bank/03-lead.toml
```

## MIDI

The Daemon has a JACK MIDI input `xguitare:midi_in`. A program change message on any channel loads the Preset with that program number: program change 0 loads Bank entry 0, 3 loads entry 3. A number beyond the Bank logs `midi program 7 out of range (bank has 5)` and changes nothing.

`midi_in` is not connected automatically. List the MIDI sources with `pw-link -o` and connect yours, e.g. the ALSA `Midi Through` port:

```sh
pw-link "Midi-Bridge:Midi Through: Port-0 (capture)" xguitare:midi_in
```

## Config

Optional file: `~/.config/xguitare/config.toml` (`$XDG_CONFIG_HOME/xguitare/config.toml`), or any path given with `xguitare --config <path> <subcommand>`. A missing file means all defaults; an unknown key is an error. Every key with its default:

```toml
bank_dir = "/home/you/.config/xguitare/bank"   # default: <config dir>/bank
buffer_size = 128                              # frames per JACK period requested at start
crossfade_ms = 20                              # Preset switch crossfade
ir_max_ms = 100                                # cabinet IRs are cut to this length
socket = "/run/user/1000/xguitare/xguitare.sock" # default: $XDG_RUNTIME_DIR/xguitare/xguitare.sock

[ports]
input = "auto"
output_left = "auto"
output_right = "auto"
```

Each `[ports]` value is one of three forms:

- `"auto"`: connect when the machine has exactly one physical capture (or playback) device; with several, connect nothing and warn with their port names.
- `""`: do not connect this port.
- A pattern: the exact JACK port name, or a name with `*` matching any run of characters. The pattern is anchored at both ends, so it must match the whole port name; the first match in sorted order wins.

A Focusrite Scarlett Solo with the guitar in input 1:

```toml
[ports]
input = "Scarlett*Input 1*capture_MONO"
output_left = "Scarlett*playback_FL"
output_right = "Scarlett*playback_FR"
```

`Scarlett*capture_FL` would match the Scarlett's loopback capture, not the guitar. `xguitare status` lists the connections made.

If `$XDG_RUNTIME_DIR` is unset, the socket defaults to `/tmp/xguitare-<uid>/xguitare.sock` and the Daemon warns.

## Troubleshooting

**Latency or buffer size.** The Daemon asks for `buffer_size` frames per period, but PipeWire may run a larger quantum. `xguitare status` shows the `buffer_size` in effect, and the Daemon warns at start:

```text
2026-11-03T12:52:17.159269Z  WARN xguitare::jack_adapter: buffer size is 256, wanted 128; force it with: pw-metadata -n settings 0 clock.force-quantum 128
2026-11-03T12:52:17.159271Z ERROR xguitare::jack_adapter: buffer size 256 exceeds prepared 128; output is silent until restart; force it with: pw-metadata -n settings 0 clock.force-quantum 128
```

The Rack is prepared for the buffer size JACK reports before the Daemon activates its client. When the quantum in effect is larger than that, output is silent: the `ERROR` line above at start, or `oversize blocks dropped` when the quantum grows later. With PipeWire this happens at start too when the quantum was already forced larger, as in the lines above (forced to 256, `buffer_size = 128`). Force the quantum for the whole PipeWire graph, restart the Daemon, and undo it with `0` when you are done playing:

```sh
pw-metadata -n settings 0 clock.force-quantum 128
pw-metadata -n settings 0 clock.force-quantum 0
```

**Clicks and dropouts (xruns).** `xguitare status` counts them in `xruns`, and the Daemon logs `xruns: N (+n)` at most once a second. `callback_max_us` is the longest audio callback so far; at 128 frames and 48 kHz a period is 2666 µs, and a callback near that is too slow. Use a larger buffer: set `buffer_size = 256` in the config, force the quantum to 256 with the command above, and restart the Daemon. Closing other audio-heavy programs or using a Preset with fewer Modules also helps.

**`error: daemon already running at /run/user/1000/xguitare/xguitare.sock`.** `xguitare run` found another Daemon answering on the socket. Stop that one (Ctrl-C in its terminal, or `pkill -x xguitare`), or use it with the CLI. A socket file left by a crashed Daemon is removed automatically.

**`error: daemon not running (socket ...)`.** A CLI command found no Daemon. Start one with `xguitare run`.

**Cabinet sounds dry, or a warning like `cabinet 'cabinet': /home/you/.config/xguitare/bank/default-cab.wav: No such file or directory (os error 2)`.** The Cabinet IR named by `ir` is missing or unreadable. The Preset still loads; that cabinet passes the signal through unchanged (mono to both outputs). Fix the `ir` path, which is relative to the Bank directory. To get the starter files back, move the Bank directory away and restart the Daemon.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
