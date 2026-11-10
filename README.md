# xguitare

![CI](https://github.com/han-xyz/xguitare/actions/workflows/ci.yml/badge.svg)

Low-latency guitar amp and effects rack for Linux, headless first.
Rust, JACK API (runs natively on PipeWire via pipewire-jack), own DSP, TOML presets.

One guitar in, stereo out. A Daemon runs the audio; the `xguitare` CLI talks to it over a Unix socket. The window [`xguitare-gui`](#gui) and any script can use the same socket: see [docs/control-api.md](docs/control-api.md). Changes between versions are in [CHANGELOG.md](CHANGELOG.md).

## Requirements

- Linux with PipeWire and `pipewire-jack` (the JACK client library PipeWire provides). xguitare is a JACK client and never starts a JACK server itself.
- A Rust toolchain, Rust 1.95 or newer (eframe 0.37, which `xguitare-gui` uses, needs 1.95), e.g. from [rustup](https://rustup.rs). The Daemon and CLI alone (`cargo install --path crates/xguitare`) build with Rust 1.88 or newer.
- For `xguitare-gui` on Linux: on Arch `libxkbcommon libxcb openssl` plus a Vulkan or OpenGL driver (wgpu picks one); on Debian/Ubuntu `libxkbcommon-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libssl-dev`. The Daemon and CLI need none of these.

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
2026-11-03T12:52:12.034094Z  INFO xguitare::daemon: created starter bank (guitar) in /home/you/.config/xguitare/bank
2026-11-03T12:52:12.034221Z  INFO jack_sys: Loading jack from libjack.so.0.
2026-11-03T12:52:12.036502Z  INFO xguitare::daemon: loaded 0 Clean
2026-11-03T12:52:12.040213Z  INFO xguitare::jack_adapter: jack client active: sample_rate=48000 buffer_size=128
2026-11-03T12:52:12.040576Z  INFO xguitare::jack_adapter: connected Scarlett Solo 4th Gen Input 1 Inst/Line:capture_MONO -> xguitare-4242:in
2026-11-03T12:52:12.040808Z  INFO xguitare::jack_adapter: connected xguitare-4242:out_l -> Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FL
2026-11-03T12:52:12.040993Z  INFO xguitare::jack_adapter: connected xguitare-4242:out_r -> Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FR
```

- `created starter bank (guitar) in ...` appears on the first run only: the Bank directory did not exist, so the Daemon created it with five starter Presets (Clean, Crunch, Lead, High Gain, Bass) and the cabinet IR `default-cab.wav`; with `bank_kind = "bass"` it creates the bass Bank instead (see [Bass](#bass)). An existing Bank directory is never touched.
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

[module.params]
level = 12.0

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
- `bypass = true` on a `[[module]]` starts that Module bypassed: it passes its input through unchanged and keeps its Parameters. Omitted means `false`. `xguitare bypass <id> on|off` switches it live, and `save` writes it.
- `cabinet` needs `ir`: the name of a Cabinet IR, a WAV file (16/24/32-bit PCM or 32-bit float, mono or stereo). The name is looked up under the IR directory (config `ir_dir`, default the Bank directory) and, when no file of that name is there, under the Bank directory. Only the first `ir_max_ms` of the file is read, and an IR with a NaN or infinite sample is refused. It must be a relative name: no absolute path, no `..` or `.`, so `irs/cab.wav` works and `../cab.wav` is an error. Every IR is normalised to unit energy on load (a stereo IR to unit energy per channel on average, so it plays as loud in total as a mono IR), so IRs sit at one loudness and `level` balances the cabinet against the rest. `xguitare ir list` shows the names in the IR directory. The cabinet turns the mono signal into stereo, so a Rack has at most one cabinet, `delay`, `chorus`, `flanger` and `reverb` come after it and every other Module before it.

Modules and Parameters (`min..max default unit`):

| Module | Parameters |
| --- | --- |
| `gate` | `threshold` −90..0 −60 dB, `attack` 0.1..50 1 ms, `release` 5..1000 100 ms, `hysteresis` 0..20 6 dB |
| `compressor` | `threshold` −60..0 −20 dB, `ratio` 1..20 4, `attack` 0.1..100 10 ms, `release` 10..1000 100 ms, `makeup` 0..24 0 dB |
| `drive` | `drive` 0..100 50 %, `tone` 0..100 50 %, `level` −30..10 0 dB |
| `fuzz` | `sustain` 0..100 60 %, `tone` 0..100 50 %, `level` −30..10 0 dB |
| `preamp` | `gain` 0..100 40 %, `bias` −1..1 0, `master` −60..0 −12 dB, `stages` 1..4 3 |
| `tonestack` | `bass`, `middle`, `treble` 0..100 50 % |
| `cabinet` | `level` −20..20 0 dB, `low_cut` 20..300 20 Hz, `high_cut` 2000..20000 20000 Hz |
| `delay` | `time` 10..2000 400 ms, `feedback` 0..95 35 %, `mix` 0..100 25 %, `pingpong` 0..1 0 |
| `chorus` | `rate` 0.1..5 1 Hz, `depth` 0..100 50 %, `mix` 0..100 40 % |
| `flanger` | `rate` 0.05..5 0.3 Hz, `depth` 0..100 60 %, `feedback` −95..95 50 %, `mix` 0..100 50 % |
| `reverb` | `size` 0..100 50 %, `damping` 0..100 50 %, `mix` 0..100 20 %, `width` 0..100 100 % |
| `gain` | `level` −60..20 0 dB |

Bank numbering: every `*.toml` file in the Bank directory, sorted by file name, numbered from 0. That number is the program number for `load` and for MIDI; prefix file names with `01-`, `02-`, ... to fix the order. A file that does not parse keeps its number and shows in `list` with `!` and the error.

Hot reload: the Daemon watches the Bank directory. Saving the active Preset's file in an editor switches to the new version with a short crossfade; if the file no longer parses, the Daemon logs the error and keeps playing the old version. Adding, removing or renaming files renumbers the Bank.

### Bass

Set `bank_kind = "bass"` in the config to seed the Bank directory (default `~/.config/xguitare/bank`) with bass Presets when it does not exist yet:

```toml
bank_kind = "bass"
```

Choosing the other kind in the GUI (the Control API's `set_settings`) keeps the Bank directory in use as that of the current kind and switches to `<config dir>/bank-<kind>` (`bank-bass` or `bank-guitar`), seeded when missing; both directories are recorded in `state.toml`, so switching back returns to your Presets. With a `bank_dir` in the config the GUI cannot switch.

The first run creates five Presets (Blues, Funk, Primus Slap, Primus Fuzz, Primus Flange) and the bass cabinet IR `bass-cab.wav`. Primus Fuzz uses the `fuzz` Module.

## CLI

Every subcommand except `run` and `tuner --watch` sends one request to the running Daemon and exits 0 on success; `tuner --watch` keeps requesting until Ctrl-C; on an error it prints one `error: ...` line to stderr and exits 1. `xguitare --help` lists them.

`status` shows the active Preset, whether it is `modified` (the live Rack differs from the Preset file), the Revision, whether Mute is on, input and output peaks since the last `status` (each client connection, such as the GUI, keeps its own), audio settings and port connections:

```console
$ xguitare status
preset: Clean
program: 0
modified: no
sample_rate: 48000
buffer_size: 128
prepared_frames: 128
revision: 1
muted: false
input_peak_db: -102.2
output_peak_db: -120.0
xruns: 0
callback_max_us: 1228
callback_last_us: 764
connections:
  Scarlett Solo 4th Gen Input 1 Inst/Line:capture_MONO -> xguitare-4242:in
  xguitare-4242:out_l -> Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FL
  xguitare-4242:out_r -> Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FR
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

`get` shows Parameters: all of them, one Module's, or one. With `get` and `get <cabinet>` a cabinet's lines end with its Cabinet IR, `cabinet.ir = default-cab.wav` (`none` when it has none); `get cabinet.level` prints only that Parameter:

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

`save --as <name>` writes them to a new Preset file in the Bank instead, named after `<name>`, and switches to it; a name another Preset already has is refused:

```console
$ xguitare save --as "My Lead Tone"
saved /home/you/.config/xguitare/bank/04-my-lead-tone.toml
```

`modules` lists the Module types with their channels and Parameters; `rack add <type> [--after <id> | --at <position>]`, `rack remove <id>` and `rack move <id> <position>` change the running Rack's layout (without `--after` or `--at` the Module goes to the end; positions count from 0). Stereo Modules go after the cabinet, mono ones before it; a layout that breaks this is refused and nothing changes. `save` writes the new layout to the Preset file. On the starter Clean Preset (gate, preamp, tonestack, cabinet, reverb):

```console
$ xguitare modules
gain        mono            level
gate        mono            threshold,attack,release,hysteresis
...
$ xguitare load Clean
loaded 0 Clean
$ xguitare rack add delay
added delay
$ xguitare rack move delay 1
error: stereo module 'delay' before the cabinet
$ xguitare rack move delay 6
moved delay to 5
$ xguitare rack remove delay
removed delay
```

`ir list` prints the Cabinet IRs in the IR directory, one name per line, and nothing when there are none. `ir set <file> [--id <module>]` loads one into the Rack's cabinet Module (`--id` names it when you need to) with a short crossfade. An IR that cannot be loaded is refused and nothing changes. `save` writes the new `ir` to the Preset file:

```console
$ xguitare ir list
cabs/v30.wav
default-cab.wav
$ xguitare ir set cabs/v30.wav
ir cabinet cabs/v30.wav
$ xguitare ir set nope.wav
error: /home/you/.config/xguitare/bank/nope.wav: No such file or directory (os error 2)
```

`tuner` prints the Tuner's reading of the dry input: the nearest note, the pitch and how many cents it is off (A4 at `tuner_reference_hz`), or `no signal`:

```console
$ xguitare tuner
E1  41.2 Hz -3 ct
```

`tuner --watch` keeps printing readings until Ctrl-C or SIGTERM: on a terminal it rewrites one line, otherwise it prints one line per reading. With `tuner_mute = true`, the default, it turns Mute on while it runs and off when it stops:

```console
$ timeout -s INT 2 xguitare tuner --watch
no signal
no signal
...
```

`mute on|off` silences both outputs, or brings them back; the Tuner keeps reading while muted:

```console
$ xguitare mute on
muted
$ xguitare mute off
unmuted
```

`bypass <id> on|off` bypasses one Module, which then passes its input through unchanged and keeps its Parameters, or switches it back in; the switch fades over 10 ms, and a Module switched back in starts from silence (delay and reverb tails restart). Like `set`, it changes the running sound until `save` writes it to the Preset:

```console
$ xguitare bypass preamp on
preamp: bypassed
$ xguitare bypass preamp off
preamp: on
```

## GUI

`xguitare-gui` is a window onto the running Daemon: it uses the same Control API as the CLI. Install it next to `xguitare`, so that it can start the Daemon:

```sh
cargo install --path crates/xguitare-gui
xguitare-gui
```

`xguitare-gui --config <path>` reads the same config file as `xguitare --config <path>`, for the socket path, and passes it on to a Daemon it starts.

The window has a left panel, the rack in the centre and a footer line at the bottom.

The left panel has these sections, from the top; click a section's title to close or open it (`Audio` and `Settings` start closed):

- `Preset`: the active Preset's name in large type, and under it `Program N` (its Bank position) and `unsaved changes` while the live Rack differs from the Preset file (`modified` in `xguitare status`). `Save` writes the live Rack to the active Preset's file, as `xguitare save` does. `Save as new` opens a name field: type a name and press Enter or the `Save` under it to write a new Preset file in the Bank directory and make it the active Preset, as `xguitare save --as <name>` does; `Cancel` closes the field. An empty or taken name shows the Daemon's error in the footer (`error: a preset named 'Funk' already exists`).
- `Guitar Bank` or `Bass Bank`, after the Bank kind: a search field, which keeps the rows whose name contains the text, in any case, or whose program number is the text, and one row per Bank entry, `program name`, the active Preset highlighted. Click a row to load it. An invalid entry is greyed out; hover over it for the parse error.
- `Rack`: `Add Module` and `Place` menus and an `Add` button (see Rack editing below).
- `Tuner and Mute`: `Tuner` opens and closes the Tuner view (see Tuner below); `Mute` shows the Daemon's Mute state and turns it on or off, as `xguitare mute` does.
- `Audio`: `Input`, `Output L` and `Output R` menus listing the ports the Daemon can connect to, each showing the port connected now. Opening a menu reads the port list again, so a device plugged in since shows. `Apply` connects the three chosen ports now and keeps them for the next start (the Control API's `set_ports`, stored in `state.toml`, see Config); it needs a port in each menu. An error, such as a port that has gone away, shows in the footer.
- `Settings`: `Bank kind` `Guitar` or `Bass`, and the `IR folder` path. `Apply` sends what changed (the Control API's `set_settings`, stored in `state.toml`): another Bank kind switches the Bank and loads its Program 0 (see [Bass](#bass)); another IR folder must be an existing directory, given as an absolute path, and is listed by the cabinet's `IR` menu from then on. An error shows in the footer, e.g. `error: bank_dir is set in config.toml; edit it there`.
- `Engine`: `Rate` (`48 kHz`), `Buffer` (`128 frames`), `Latency` (one buffer, `2.7 ms`), `Xruns`, and `Daemon` `connected` or `disconnected`.

The rack is one 19-inch unit per row between two rails; a closed unit is 64 pixels high per row of knobs (its knobs wrap to another row when the window is too narrow for them), an open unit 190, and the rack scrolls when they do not fit. The window opens at 1280×800 and cannot be made smaller than 1024×640:

- The meter bridge: `Input` and `Output` LED ladders from -60 dB to 0 dB with the peak beside them. The top segment is the clip lamp; it stays lit after a peak of 0 dB or more until you click a ladder.
- The Tuner unit (see Tuner below).
- One unit per Module in Rack order. A closed unit has a lamp in its Module type's colour, the Module id, one small knob per Parameter with its name and value field, and `↑` `↓` at the right. Click the id or the `▶` beside it to open the unit: an open unit shows large knobs, each with its value field below, and at the right the cabinet's `IR` menu and `Remove`. Any number of units can be open; which ones is kept across restarts. Drag a unit by its silver ears to move it to the unit under the pointer; near the top or bottom of the Rack view the Rack scrolls.

The footer shows `disconnected Ns` or `starting daemon Ns` while no Daemon answers, and for 3 seconds the result of a save (`saved /home/you/.config/xguitare/bank/01-clean.toml`), the error of any command (`error: ...`) or of a typed value that is no number (`error: 'loud' is not a number`).

Knobs: drag up to raise a value and down to lower it; 220 pixels cross the whole range. Hold Shift while dragging for steps ten times finer. Double-click a knob to reset the Parameter to its default. `preamp.stages` and `delay.pingpong` step in whole numbers. The sound changes while you drag: the GUI sends `set` at most every 30 ms and the final value on release. As with `xguitare set`, a change is not written to the Preset file until you save.

Values are shown with the unit, a whole value without decimals, others with one: `−24 dB`, `5 ms`, `0.8 Hz`, `20 kHz`, a compressor ratio as `6 : 1`.

Typed values: every knob has a value field. Click it, or Ctrl+click or right-click the knob, and the field holds the bare number in the Parameter's unit (`20000` for `20 kHz`); type a value (`-8`, `12.5`, `12 dB`; `2 kHz` or `2k` is 2000) and press `Enter` to send it. `Esc` or a click elsewhere puts the shown value back; text that is no number does too, with a footer message. A value out of range is clamped by the Daemon, as with `xguitare set`.

Knobs from the keyboard: `Tab` moves to the next knob, which shows a ring, then to its value field. `↑` and `↓` move a focused knob's value by 1 % of the range (1 for a whole-number Parameter), `Page Up` and `Page Down` by 10 %, `Home` and `End` to the minimum and maximum. The `1` to `9` keys and Ctrl+S work while a knob has focus, but not while a value field has the text cursor; `Esc` releases the focus.

Rack editing, as with `xguitare rack`: in the left panel's `Rack` section, the `Add Module` menu lists the Module types with their channels, and `Place` shows where `Add` puts the chosen type: a stereo Module at the end of the Rack, a mono Module or a cabinet after the last mono Module (`at the front` when there is none). Choose another `Place` (`at the front` or `after <id>`) to add it there; the Daemon refuses a place the Rack rules forbid. `↑` and `↓` move a Module one place, `Remove` in its open unit removes it; a Remove the Daemon refuses leaves the unit open. A layout the Daemon refuses (a stereo Module before the cabinet, say) leaves the Rack unchanged and shows the error in the footer. `Save` writes the new layout to the Preset file.

Cabinet IR: an open cabinet unit has an `IR` row below its knobs, a menu showing the cabinet's current Cabinet IR (`none` when it has none). Opening the menu reads the IR directory again, so it lists the same names as `xguitare ir list`, including files copied in since. Choosing a name loads that IR into the cabinet with a crossfade, as `xguitare ir set` does; an IR that cannot be loaded leaves the cabinet unchanged and shows the error in the footer. `Save` writes the new `ir` to the Preset file. The cabinet's `low_cut` and `high_cut` are Parameters, so they are knobs on the same unit.

Tuner: the Tuner unit's header shows the note and cents (`E1  -3 ct`), or `no signal` without a confident reading, whether the unit is open or closed; the `Tuner` button in the left panel opens it to the Tuner view: the note name in large type (`no signal` without a confident reading), a half-circle scale from -50 cents (left) to +50 cents (right) with a needle, and the frequency (`41.2 Hz`) under it, green within 3 cents, amber beyond. When the config key `tuner_mute` is true (the default), opening the view turns Mute on, and pressing `Tuner` again or closing the window turns it off. Turning Mute off yourself with the `Mute` button means closing the view leaves Mute off. The `1` to `9` keys still load programs while the view is open.

Changes made elsewhere (the CLI, MIDI program change, an edited Preset file) show up at the next `status` poll, every 50 ms.

Keys: `1` to `9` load programs 0 to 8 (a key for a missing or invalid entry does nothing); `Ctrl+S` saves, like the `Save` button. Keys do nothing while a text field (search, Preset name, value, IR folder) has the text cursor.

When no Daemon answers on the socket, the GUI starts `xguitare run` itself, taken from the directory `xguitare-gui` is in, else from `PATH`, and shows `starting daemon` until it answers. The Daemon's log goes to the GUI's stderr. A Daemon the GUI started is stopped when the window closes; one you started yourself keeps running. If the GUI crashes or is killed, the Daemon it started keeps running and the sound does not stop; the next `xguitare-gui` connects to it. To stop that Daemon, send it SIGTERM: `kill <pid>`, with the pid from the GUI's `starting daemon ... (pid N)` log line or from `pgrep -x xguitare`. If the Daemon goes away, the left panel's `Engine` section shows `Daemon` `disconnected`, the footer `disconnected Ns`, and the GUI keeps reconnecting.

Window size and position are kept in `~/.local/share/xguitare/app.ron` (`$XDG_DATA_HOME/xguitare/app.ron`). `RUST_LOG` sets the GUI's log level as for the Daemon.

## MIDI

The Daemon has a JACK MIDI input `xguitare:midi_in`. A program change message on any channel loads the Preset with that program number: program change 0 loads Bank entry 0, 3 loads entry 3. A number beyond the Bank logs `midi program 7 out of range (bank has 5)` and changes nothing.

`midi_in` is not connected automatically. List the MIDI sources with `pw-link -o` and connect yours, e.g. the ALSA `Midi Through` port:

```sh
pw-link "Midi-Bridge:Midi Through: Port-0 (capture)" xguitare:midi_in
```

## Config

Optional file: `~/.config/xguitare/config.toml` (`$XDG_CONFIG_HOME/xguitare/config.toml`), or any path given with `xguitare --config <path> <subcommand>`. A missing default file means all defaults; a `--config` path must exist (`<path>: not found` otherwise); an unknown key is an error. Every key with its default:

```toml
bank_dir = "/home/you/.config/xguitare/bank"   # default: <config dir>/bank
bank_kind = "guitar"                           # starter Bank for a missing bank_dir: "guitar" or "bass"
ir_dir = "/home/you/.config/xguitare/bank"     # default: the Bank directory; where Preset `ir` names are looked up first
buffer_size = 128                              # frames per JACK period requested at start, 1 to 8192
crossfade_ms = 20                              # Preset switch crossfade, above 0 to 1000
ir_max_ms = 100                                # cabinet IRs are cut to this length, above 0 to 10000
tuner_reference_hz = 440.0                     # A4 for the Tuner, 400–480
tuner_mute = true                              # the GUI's Tuner view and `tuner --watch` mute the output
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

`<config dir>` is the directory holding the config file in use, also for `--config <path>`.

The Daemon keeps the choices made in the GUI (or through the Control API's `set_ports` and `set_settings`) in `state.toml` beside the config file, mode 0600, and never rewrites `config.toml`, so its comments survive. `state.toml` is read after `config.toml` and overrides it: `[ports]` (`input`, `output_left`, `output_right`, exact port names), `bank_kind`, `ir_dir`, and `[banks]` (`guitar`, `bass`: each kind's Bank directory, recorded on the first switch; the one of `bank_kind` is the Bank directory). A `bank_dir` in `config.toml` is never overridden, `state.toml`'s `bank_kind` is ignored while it is set, and the GUI cannot switch the Bank kind while it is set. If `state.toml` cannot be parsed, the Daemon logs `<path>: line N:M: <error>; ignoring it` and starts with `config.toml` alone. To drop the GUI's choices, stop the Daemon and delete `state.toml`.

If `$XDG_RUNTIME_DIR` is unset, the socket defaults to `/tmp/xguitare-<uid>/xguitare.sock` and the Daemon warns.

## Troubleshooting

**Latency or buffer size.** The Daemon asks for `buffer_size` frames per period, but PipeWire may run a larger quantum. `xguitare status` shows the `buffer_size` in effect, and the Daemon warns at start:

```text
2026-11-03T15:58:05.926714Z  WARN xguitare::jack_adapter: buffer size is 256, wanted 128; force it with: pw-metadata -n settings 0 clock.force-quantum 128
2026-11-03T15:58:05.926719Z  INFO xguitare::jack_adapter: buffer size 256 differs from prepared 128; re-preparing
2026-11-03T15:58:06.928152Z  WARN xguitare::daemon: 186 oversize blocks dropped: quantum 256, rack running at 128 frames; re-preparing
2026-11-03T15:58:06.929391Z  INFO xguitare::daemon: re-prepared rack for 256 frames
```

The Rack is prepared for the buffer size JACK reports before the Daemon activates its client. When the quantum in effect differs, at start or later, the Daemon rebuilds the Rack for the new size within a second and logs `re-prepared rack for N frames`; `xguitare status` shows that size as `prepared_frames`. Sound continues with no restart, after at most a second of silence when the quantum grew (logged as `oversize blocks dropped`). To get the latency of `buffer_size`, force the quantum for the whole PipeWire graph, and undo it with `0` when you are done playing:

```sh
pw-metadata -n settings 0 clock.force-quantum 128
pw-metadata -n settings 0 clock.force-quantum 0
```

When the audio server changes the sample rate while the Daemon runs, the Daemon rebuilds the Rack (Cabinet IRs resampled) and the Tuner at the new rate within 100 ms and logs `sample rate changed: 48000 -> 96000`; `xguitare status` shows the new `sample_rate`.

**Clicks and dropouts (xruns).** `xguitare status` counts them in `xruns`, and the Daemon logs `xruns: N (+n)` at most once a second. `callback_max_us` is the longest audio callback so far; at 128 frames and 48 kHz a period is 2666 µs, and a callback near that is too slow. Use a larger buffer: force the quantum to 256 with the command above; the Daemon re-prepares within a second. Set `buffer_size = 256` in the config so the next start asks for it. Closing other audio-heavy programs or using a Preset with fewer Modules also helps.

**`error: another Daemon is running`.** `xguitare run` found another Daemon holding the socket's lock file (`xguitare.sock.lock` beside the socket). Stop that one (Ctrl-C in its terminal, or `pkill -x xguitare`), or use it with the CLI. A socket file left by a crashed Daemon is removed automatically.

**`error: daemon not running (socket ...)`.** A CLI command found no Daemon. Start one with `xguitare run`.

**Silent output after a tuner client died.** Mute stays on when the client that turned it on is killed before it can turn it off; `xguitare status` shows `muted: true`. Run `xguitare mute off`.

**Cabinet sounds dry, or a warning like `cabinet 'cabinet': /home/you/.config/xguitare/bank/default-cab.wav: No such file or directory (os error 2)`.** The Cabinet IR named by `ir` is missing or unreadable. The Preset still loads; that cabinet passes the signal through unchanged (mono to both outputs). Fix the `ir` name, which is looked up under the IR directory (`ir_dir`) and then the Bank directory. To get the starter files back, move the Bank directory away and restart the Daemon.

**`ir set` answers `error: <path>: Failed to read enough bytes` or another read error.** The IR must be a 16/24/32-bit PCM or float wav with one or two channels.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
