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

### Install from a release

Each tagged release on GitHub carries `xguitare-<version>-x86_64-linux.tar.gz` and a `.sha256` file. It holds the Daemon and CLI `xguitare` and the window `xguitare-gui`; the starter Banks are built in.

```sh
sha256sum -c xguitare-<version>-x86_64-linux.tar.gz.sha256
tar -xzf xguitare-<version>-x86_64-linux.tar.gz
cd xguitare-<version>
./xguitare run
```

It needs PipeWire with `pipewire-jack` (libjack) at run time, and for `xguitare-gui` the libraries listed under Requirements.

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

- `created starter bank (guitar) in ...` appears on the first run only: the Bank directory did not exist, so the Daemon created it with eight starter Presets (Clean, Crunch, Lead, High Gain, Bass, Rock, Blues, Funk) and the cabinet IR `default-cab.wav`; with `bank_kind = "bass"` it creates the bass Bank instead (see [Bass](#bass)). An existing Bank directory is never touched.
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

Modules and Parameters (`min..max default unit`). In `autowah` the bp mode peaks at 0 dB at its cutoff whatever `q`; lp and hp keep a resonant peak, up to about +22 dB at `q` 12, so lower `wet` or `q` when they get loud. A choice Parameter lists its choices instead of a range (`mode` lp|bp|hp): a Preset and `set` take the choice's number, counted from 0, so `mode = 1` is bp. `blend` runs its whole input, the clean part included, through 4x oversampling: it adds about 0.5 ms of delay (22.5 samples at 48 kHz), which is not reported as latency, and rolls off slightly above 15 kHz (−1.7 dB at 20 kHz). `limiter` delays the sound by `lookahead` ms so that it can lower the gain before a peak arrives; that delay is not counted in recording latency compensation. Its output never goes above `ceiling`. `shaper` raises or lowers the start of each note (`attack`) and what follows it (`sustain`) whatever the input level; at `attack` 0 and `sustain` 0 it leaves the sound unchanged.

| Module | Parameters |
| --- | --- |
| `gate` | `threshold` −90..0 −60 dB, `attack` 0.1..50 1 ms, `release` 5..1000 100 ms, `hysteresis` 0..20 6 dB |
| `compressor` | `threshold` −60..0 −20 dB, `ratio` 1..20 4, `attack` 0.1..100 10 ms, `release` 10..1000 100 ms, `makeup` 0..24 0 dB |
| `shaper` | `attack` −15..15 0 dB, `sustain` −24..24 0 dB, `output` −12..12 0 dB, `mix` 0..100 100 % |
| `autowah` | `mode` lp\|bp\|hp 0, `direction` up\|down 0, `sensitivity` −24..36 0 dB, `attack` 0.5..50 5 ms, `decay` 20..1000 150 ms, `freq_low` 80..800 200 Hz, `freq_high` 400..4000 2500 Hz, `q` 0.5..12 3, `dry` 0..100 50 %, `wet` 0..100 100 % |
| `drive` | `drive` 0..100 50 %, `tone` 0..100 50 %, `level` −30..10 0 dB |
| `fuzz` | `sustain` 0..100 60 %, `tone` 0..100 50 %, `level` −30..10 0 dB |
| `blend` | `blend` 0..100 100 %, `drive` 0..100 40 %, `level` −24..12 0 dB, `bass` −12..12 0 dB, `bass_freq` 40 Hz\|80 Hz 1, `mid` −12..12 0 dB, `mid_freq` 500 Hz\|1000 Hz 0, `treble` −12..12 0 dB, `presence` 0..100 50 %, `crossover` 20..400 20 Hz |
| `preamp` | `gain` 0..100 40 %, `bias` −1..1 0, `master` −60..0 −12 dB, `stages` 1..4 3 |
| `tonestack` | `bass`, `middle`, `treble` 0..100 50 % |
| `eq` | `low_cut` 10..300 10 Hz, `low_freq` 20..500 100 Hz, `low_gain` −15..15 0 dB, `mid1_freq` 40..5000 400 Hz, `mid1_gain` −15..15 0 dB, `mid1_q` 0.3..10 0.707, `mid2_freq` 200..10000 2500 Hz, `mid2_gain` −15..15 0 dB, `mid2_q` 0.3..10 0.707, `high_freq` 1000..20000 8000 Hz, `high_gain` −15..15 0 dB, `high_cut` 1000..20000 20000 Hz, `output` −12..12 0 dB |
| `limiter` | `input` 0..18 0 dB, `ceiling` −12..0 −1 dB, `release` 10..1000 100 ms, `lookahead` 0..5 1 ms |
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

The first run creates eight Presets (Blues, Funk, Primus Slap, Primus Fuzz, Primus Flange, Rock, Envelope Funk, Modern Grind) and the bass cabinet IR `bass-cab.wav`. Primus Fuzz uses the `fuzz` Module, Envelope Funk `autowah` and `eq`, Modern Grind `eq`, `blend` and `limiter`. In a Preset's Rack the `limiter` is a mono Module and sits before the cabinet, so the cabinet's `level` sets the final peak; a limiter that is truly the last Module belongs on a Bus or the Master, which are stereo.

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
disk_overflows: 0
metronome: off, 120 bpm 4/4, volume -12 dB, beat 0
project: -
```

While a Project is open, `status` goes on with the transport, the playhead, tempo, loop, click, one line per Track, one per Bus and a Master line (see Projects).

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

`get` shows Parameters: all of them, one Module's, or one. With `get` and `get <cabinet>` a cabinet's lines end with its Cabinet IR, `cabinet.ir = default-cab.wav` (`none` when it has none); then every Module's lines end with `<id>.bypass = yes|no`. `get preamp.gain` prints only that Parameter:

```console
$ xguitare get preamp
preamp.gain = 100 %
preamp.bias = 0.2
preamp.master = -14 dB
preamp.stages = 3
preamp.bypass = no
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

`modules` lists the Module types with their channels and Parameters, a choice Parameter with its choices (`mode (lp|bp|hp)`); `rack add <type> [--after <id> | --at <position>]`, `rack remove <id>` and `rack move <id> <position>` change the running Rack's layout (without `--after` or `--at` the Module goes to the end; positions count from 0). Stereo Modules go after the cabinet, mono ones before it; a layout that breaks this is refused and nothing changes. `save` writes the new layout to the Preset file. On the starter Clean Preset (gate, preamp, tonestack, cabinet, reverb):

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

## Metronome

The Metronome is a short blip on every beat, beat 1 of each bar higher and 6 dB louder, mixed into both outputs. It works with no Project open, over the Rack:

```console
$ xguitare metronome on
metronome on, 120 bpm 4/4, volume -12 dB
$ xguitare metronome tempo 90 3/4
metronome on, 90 bpm 3/4, volume -12 dB
$ xguitare metronome volume -20
metronome on, 90 bpm 3/4, volume -20 dB
$ xguitare metronome tap
tap
$ xguitare metronome tap
tap 118.5 bpm
$ xguitare metronome off
metronome off, 118.5 bpm 3/4, volume -20 dB
```

- `metronome on` starts on beat 1 of a bar; `metronome tempo <bpm> [<beats>/<note>]` (20 to 300 bpm, decimals allowed, the meter as a Project's) keeps the beat count; `metronome volume <dB>` takes −40 to 0 (default −12).
- `metronome tap` taps the tempo: from the second tap the tempo is the average interval of the last taps, up to four; a pause over 2 seconds starts a new series.
- Mute silences it; it keeps counting, so it comes back in time.
- Its tempo, meter and volume are kept in `state.toml` for the next start; it always starts off.
- `status` shows it on its `metronome:` line, with the beat in the bar it last played.

While a Project is open, the Metronome is the Project's click: `metronome on|off` is `click on|off`, `metronome tempo` and `metronome tap` set the Project's tempo and meter, and the volume and the accent on beat 1 apply to the click and count-in. Opening a Project switches the Metronome off, and it stays off after the Project closes; its own tempo and meter come back then.

In the GUI, the left panel's `Metronome` section holds the same: an `On`/`Off` switch, lit yellow while on; the beat light, which shows the beat in the bar and flashes on each new beat, yellow on beat 1 and white on the others, and stays dark while the Metronome is off; `Tap`; the tempo field (`90.0 bpm`, sent on Enter, when the field loses focus or when a drag ends); the meter menu (2/4, 3/4, 4/4, 5/4, 6/8, 7/8, 12/8); and the `Volume` knob with its value field (−40 to 0 dB; a double-click on the knob goes back to −12 dB). With no Project open, `M` switches the Metronome on or off and `T` taps the tempo; neither acts while a text field has the text cursor. While a Project is open, the section shows the Project's tempo and meter, `On`/`Off` is the Project's click, a line under it reads `follows the Project`, and `M` and `T` do nothing.

## Projects

The GUI's Project view (see [Project view](#project-view)) does all of this from the window.

A Project is a recording session: Tracks, each with its own Rack, input, volume, pan, mute, solo and arm, mixed through Buses and Master (see [Mixer](#mixer)) with a click to the stereo output. While one is open the Daemon plays it instead of the active Preset; closing it brings the Preset back. Presets, the Bank and MIDI program change work as before when no Project is open; with one open, `load`, `set`, `get`, `bypass`, `rack` and `ir set` edit the selected Track (`track select`) instead, and `set`, `get`, `bypass`, `rack` and `ir set` take `--bus <bus>` or `--master` to edit a Bus's or the Master Rack.

### Folder layout

Projects live under `projects_dir`, by default `~/Music/xguitare`; the GUI's `Settings` section (`Projects folder`) or the Control API `set_settings` changes it (an absolute path, stored in `state.toml`). Each Project is one folder named after the Project (lowercase, letters and digits, `-` between words):

```text
~/Music/xguitare/demo-song/
  project.toml            name, sample rate, tempo, meter, loop, click, count-in, Master, Buses, Tracks with their Modules, Sends, Items and Takes
  project.autosave.toml   the latest autosave, when there is one
  takes/guitar-1.wav      one mono 32-bit float WAV per Take, at the Project's sample rate
  renders/demo-song.wav   each Render, a stereo WAV (see Render)
```

Takes are never deleted, also not when their Track is removed. `project.toml` names each Take file `takes/<name>.wav`; any other path is an error when the Project is opened or saved.

### Record a Track

```console
$ xguitare project new Live
created Live at /home/you/Music/xguitare/live
$ xguitare track add Guitar
added track 0 Guitar
$ xguitare track edit 0 --input "Scarlett Solo 4th Gen Input 1 Inst/Line:capture_MONO" --arm on
track 0 Guitar: input Scarlett Solo 4th Gen Input 1 Inst/Line:capture_MONO, volume 0 dB, pan 0, mute no, solo no, arm yes
$ xguitare record
recording
$ xguitare stop
stopped
$ xguitare status
...
project: Live
transport: stopped
position: 2.3 (frame 148736)
tempo: 120 bpm 4/4
loop: off 0..0
click off, count-in 0 bars
tracks:
  0* Guitar: armed; input Scarlett Solo 4th Gen Input 1 Inst/Line:capture_MONO; 0 dB, pan 0; peak -88.8 dB; out -91.8/-91.8 dB
buses:
master: 0 dB; out -91.8/-91.8 dB
$ xguitare project save
saved /home/you/Music/xguitare/live/project.toml
$ xguitare project close
closed Live
```

- `project new|open|save|close|list|items`: `open <name> --restore` loads the autosave; `close` saves, and saves again once the Takes still being written are added (after a recording, a `project new` or `open` meanwhile answers `a project is still closing; try again`). A sample rate other than the audio server's is refused; if the audio server's rate changes while a Project is open, the Project stops and `play` and `record` are refused until the rates match again.
- `project items` prints one line per Item: its Track, where it starts as `<bar>.<beat>`, its length in seconds (its active Take's frames minus the frames its `offset` skips) and which Take plays, e.g. `Guitar 2.1 4.00s take 2/2`. Right after `stop` the new Takes are not listed yet: they show up about a second later, once the disk thread has finished writing them.
- `track add [name]` adds a Track with the active Preset's Modules and selects it. `track remove|select <track>` and `track edit <track>` take a zero-based index or a name (a name cannot be only digits); `edit` takes `--input <port>` (`""` disconnects; port names as the Control API `ports` lists them), `--volume <dB>` (−60 to 6), `--pan <−1..1>`, `--mute|--solo|--arm on|off`, `--to-master on|off` and `--name`.
- An armed Track monitors its input through its Rack, also while stopped, and records while recording; an unarmed Track plays its Items.
- `play`, `stop` (the playhead stays), `record` (from stopped, after the count-in) and `locate <bar>.<beat>|<bar>|<seconds>s`, e.g. `locate 3.2` or `locate 12.5s`, up to an hour past the last Take or the loop end.
- `tempo <bpm> [<beats>/<note>]`, e.g. `tempo 100 3/4`; `click on|off [--count-in 0|1|2]`; all are saved with the Project. The loop region is set through the Control API `loop`, or in the GUI by dragging its edges on the ruler or pressing `R`.
- While recording, a Track cannot be removed and an armed Track's input cannot change; `stop` first.

Each Track records the dry input; its Rack is applied on playback, so its sound can still change after recording. A recording becomes a Take on a new Item; with the loop on, each pass after a wrap becomes another Take of the same Item, the newest one playing.

### Mixer

Each Track's sound goes, after its Rack, volume and pan, to Master (unless `to master` is off) and through a Send to each Bus. A Bus is a stereo Rack with a volume, pan, mute and solo, and goes to Master; Master is a stereo Rack with a volume, and goes to the output. The click is added after Master and does not follow its volume.

```console
$ xguitare bus add Verb
added bus 0 Verb
$ xguitare rack add reverb --bus Verb
added reverb
$ xguitare send 0 Verb -6
send track 0 -> bus 0: -6 dB, post
$ xguitare master -3
master -3 dB
$ xguitare status
...
tracks:
  0  Guitar: -; input -; 0 dB, pan 0; peak -120.0 dB; out -120.0/-120.0 dB
      send Verb: -6 dB, post
  1* Keys: -; input -; 0 dB, pan 0; peak -120.0 dB; out -120.0/-120.0 dB
buses:
  0 Verb: -; 0 dB, pan 0; out -120.0/-120.0 dB
master: -3 dB; out -120.0/-120.0 dB
```

- `bus add [name]` adds a Bus with an empty Rack at 0 dB, centred; without a name it is `Bus <n>`, the first number free. A Project has at most 8 Buses; Bus names follow the Track name rules. `bus remove <bus>` removes it and every Send to it. `bus edit <bus>` takes `--name`, `--volume <dB>` (−60 to 6), `--pan <−1..1>` and `--mute|--solo on|off`. A Bus is named by its zero-based index or its name.
- `send <track> <bus> <level>` sets a Track's Send to a Bus: a level in dB (−60 to 6) or `off`, the default. A Send is taken after the Track's volume and pan (post-fader); `--pre on` takes it after the Rack and pan, before the volume.
- `master <dB>|off` sets the Master volume (−60 to 6, or off). `status` shows an off Master as `master: - dB; …`, and an off Send as `send <bus>: - dB, <pre|post>`.
- `track edit <track> --to-master off` keeps a Track's own sound out of Master, so it is heard only through its Sends.
- `rack add|remove|move`, `set`, `get`, `bypass` and `ir set` take `--bus <bus>` or `--master` to edit that Rack instead of the selected Track's; a Bus or Master Rack takes every Module type but the cabinet (`a cabinet is not allowed on a Bus or Master`), in any order, and `load` puts a Preset only into a Track.
- Solo is solo in place: while anything is soloed, the soloed Tracks and Buses sound, with the Buses a soloed Track sends to and the Tracks that send to a soloed Bus. Mute always wins.
- `status` shows each Track's, Bus's and Master's output peaks (left/right, after the volume) since the last `status`, and `clip` once a side has reached 0 dBFS. The clip flag stays until `clip reset` (all), `clip reset track <track>`, `clip reset bus <bus>` or `clip reset master`. Removing a Track or Bus clears the clip flags of the Tracks or Buses after it.

### Render

A Render plays the Project through its Tracks, Buses and Master into a stereo WAV at the Project's sample rate, faster than real time, while the Daemon goes on playing and recording. It renders the Project as it is now, unsaved changes included; the click and the live inputs are left out, and an armed Track renders its Items.

```console
$ xguitare render
rendering /home/you/Music/xguitare/live/renders/live.wav
10%
40%
70%
rendered /home/you/Music/xguitare/live/renders/live.wav
$ xguitare render --bits 16
rendering /home/you/Music/xguitare/live/renders/live-2.wav
...
rendered /home/you/Music/xguitare/live/renders/live-2.wav
```

- `render` waits until the Render ends, printing `NN%` for each further tenth it sees while polling (a short Render may skip some, and `100%`), and ends with `rendered <file>`; a Render that fails or is cancelled ends with `error: render failed: <reason>` or `error: render cancelled` and exit 1. `render cancel`, from another shell, stops it; so do closing the Project and stopping the Daemon.
- The whole Project renders from the start to the end of its last Item, then on until Master has stayed below −90 dBFS for half a second (a reverb or delay tail), 10 seconds at most. `--loop` renders the loop region exactly instead.
- `--bits 24` (default), `16` (with dither) or `32f` (32-bit float).
- The file is `renders/<name>.wav` in the Project folder, `<name>` being `--name` or the Project folder's name. An existing file is never replaced: `-2`, `-3`, … is added. A name must not be empty, start with `.` or contain `/` or `..`. While it runs the Render writes `<file>.part`, removed when it is cancelled or fails.
- One Render runs at a time; `status` shows the running one, or the last, on its `render:` line.

### Latency compensation

What you play reaches the Daemon late by the input's capture latency, and you play along to what you hear late by the output's playback latency. Each Take is placed earlier by both, as JACK reports them for the Track's input port and for the port `out_l` feeds, plus the Project's `latency_offset_ms` (−1000 to 1000, edited in `project.toml`) to correct what the drivers do not report. A Take recorded from frame 0 starts at frame 0 with its first frames skipped (the Item's `offset`).

### Autosave

While a Project is open and changed, the Daemon writes `project.autosave.toml` every 60 seconds, and when it stops (after adding the Takes being recorded); `project.toml` changes only on `project save` and `project close`. When the autosave is newer, `project open` says `its autosave is newer; open it with --restore to load the autosave`.

### Limits of this version

- Takes are held in memory while the Project is open: about 11.5 MB per minute per Track at 48 kHz.
- When the disk falls behind, the dropped frames are counted in `status` `disk_overflows` and left out of the Take, which then plays shorter and early from that point; they are not filled with silence.
- With the loop on, a recording that does not start at the loop start puts its first pass and the passes after the first wrap on separate Items.
- A Take that cannot be written or loaded is only reported in the Daemon's log.
- Only the active Take of an Item plays; choosing another Take, moving Items and editing are not in this version.
- The GUI shows Items as blocks labelled with their Take's name, without a waveform; the waveform overview comes with Item editing.

## GUI

`xguitare-gui` is a window onto the running Daemon: it uses the same Control API as the CLI. Install it next to `xguitare`, so that it can start the Daemon:

```sh
cargo install --path crates/xguitare-gui
xguitare-gui
```

`xguitare-gui --config <path>` reads the same config file as `xguitare --config <path>`, for the socket path, and passes it on to a Daemon it starts.

The window has a left panel, the rack in the centre and a footer line at the bottom.

The left panel has these sections, from the top, each in a framed box; click a section's title strip to close or open it (`Audio` and `Settings` start closed):

- `Project`: with no Project open, `No Project open`, a `Name` field with `New` (creates the Project and opens it, as `xguitare project new` does) and `Open a Project`, a menu of the Projects in `projects_dir`, read again when it opens; choosing one opens it, and an invalid one is greyed out with its error on hover. With a Project open, its name, `audio N MB` (the Take audio it holds in memory), `Save` (`xguitare project save`) and `Close` (`xguitare project close`). See [Project view](#project-view).
- `Preset`: the active Preset's name in large type, and under it `Program N` (its Bank position) and `unsaved changes` while the live Rack differs from the Preset file (`modified` in `xguitare status`). `Save` writes the live Rack to the active Preset's file, as `xguitare save` does. `Save as new` opens a name field: type a name and press Enter or the `Save` under it to write a new Preset file in the Bank directory and make it the active Preset, as `xguitare save --as <name>` does; `Cancel` closes the field. An empty or taken name shows the Daemon's error in the footer (`error: a preset named 'Funk' already exists`).
- `Guitar Bank` or `Bass Bank`, after the Bank kind: a search field, which keeps the rows whose name contains the text, in any case, or whose program number is the text, and one row per Bank entry, `program name`, the active Preset highlighted. Click a row to load it. An invalid entry is greyed out; hover over it for the parse error.
- `Rack`: `Add Module`, one tile per Module type in its type's colour, and a `Place` menu (see Rack editing below).
- `Tuner and Mute`: `Tuner` opens and closes the Tuner view (see Tuner below); `Mute` shows the Daemon's Mute state, lit red while Mute is on, and turns it on or off, as `xguitare mute` does.
- `Metronome`: `On`/`Off`, the beat light, `Tap`, the tempo, the meter and the volume (see [Metronome](#metronome)).
- `Audio`: `Input`, `Output L` and `Output R` menus listing the ports the Daemon can connect to, each showing the port connected now. Opening a menu reads the port list again, so a device plugged in since shows. `Apply` connects the three chosen ports now and keeps them for the next start (the Control API's `set_ports`, stored in `state.toml`, see Config); it needs a port in each menu. An error, such as a port that has gone away, shows in the footer.
- `Settings`: `Bank kind` `Guitar` or `Bass`, the `IR folder` path and the `Projects folder` (`projects_dir`, an absolute path). `Apply` sends what changed (the Control API's `set_settings`, stored in `state.toml`): another Bank kind switches the Bank and loads its Program 0 (see [Bass](#bass)); another IR folder must be an existing directory, given as an absolute path, and is listed by the cabinet's `IR` menu from then on. An error shows in the footer, e.g. `error: bank_dir is set in config.toml; edit it there`.
- `Engine`: `Rate` (`48 kHz`), `Buffer` (`128 frames`), `Latency` (one buffer, `2.7 ms`), `Xruns`, and `Daemon` `connected` or `disconnected`.

While Mute is on, a red banner above the rack reads `Output muted — press Mute to unmute`, or `Output muted — close the Tuner to unmute` when the Tuner view turned Mute on.

The rack is one 19-inch unit per row between two rails; a closed unit is 64 pixels high per row of knobs (its knobs wrap to another row when the window is too narrow for them: three to a row at 1024×640), an open unit at least 168, taller when its knobs wrap to more rows, and the rack scrolls when they do not fit. The window opens at 1280×800 and cannot be made smaller than 1024×640:

- The meter bridge: `Input` and `Output` LED ladders from -60 dB to 0 dB over a dB scale (−48 to 0), with the held peak at the right. The top segment is the clip lamp; it stays lit after a peak of 0 dB or more until you click a ladder.
- The Tuner unit (see Tuner below).
- One unit per Module in Rack order, its Rack position (1 at the top, as `xguitare rack move` counts) engraved on both ears and a 2-pixel top edge in its Module type's colour. A closed unit has an `On`/`Off` switch with a lamp in its Module type's colour, the Module id, one small knob per Parameter with its name and value field, and `↑` `↓` at the right. Click the id or the `▶` after it to open the unit: an open unit shows large knobs, each with its value field below, all on one row at 1280×800 (ten for blend), wrapping to another row only between groups of Parameters that belong together (autowah: mode and direction; sensitivity, attack and decay; the two frequencies and `q`; dry and wet; blend: blend, drive and level; bass, mid and their frequencies; treble, presence and crossover), `Remove` beside `↑` `↓`, and at the right the cabinet's `IR` menu. A choice Parameter (autowah `mode`, `direction`; blend `bass_freq`, `mid_freq`) shows one button per choice in an open unit, the current one lit in the unit's colour: click one, or focus the group with `Tab` (each group is one `Tab` stop, on the current choice) and move with the arrow keys, to switch. A closed unit shows only the current choice: click it to switch a two-way choice (`Up`/`Down`) to the other, or to open the unit for one with more. The `eq` unit, named `EQ`, draws its response curve instead: closed, a small curve in place of its knobs and a `Bands` line listing each band that cuts or boosts (`+4 @100 −3 @400 +2 @2.5k`, or `flat`); open, a plot from 10 Hz to 20 kHz and −18 to +18 dB with `output` included, a dot on the curve per band (half a dot when a cut sits at the end of the axis, a ring around the dot of the band whose knob the pointer is on or that has the focus), beside its columns of small knobs (four abreast, two by two in a narrow window): Low (`low_cut`, `low_freq`, `low_gain`), Mid 1, Mid 2 (frequency, gain, `q`) and High (`high_freq`, `high_gain`, `high_cut`, `output`). Past ±18 dB the curve runs along the plot's edge. While the Daemon is disconnected the plot is empty. Any number of units can be open; which ones is kept across restarts. Click `On` to bypass the Module, as `xguitare bypass <id> on` does: the switch shows `Off` with its lamp out, the unit turns a darker grey with its text, knob arcs and lit choices in grey, and a `Bypassed` tag sits after the `▶`; its knobs still change its Parameters. Click `Off` to turn it back on. `Save` writes the bypass to the Preset file. Drag a unit by its header strip (the `⠿` grip beside the left ear marks it) or by its silver ears: a line in the unit's colour marks the gap it drops into, release to move it there; near the top or bottom of the Rack view the Rack scrolls. Release it below the last unit to move it to the end. Release it anywhere else outside the rack area (the left panel, the footer, the mute banner or the margin around the rack) to remove the Module; release it outside the window to leave it where it was.

The footer shows `disconnected Ns` or `starting daemon Ns` while no Daemon answers, and for 3 seconds the result of a save (`saved /home/you/.config/xguitare/bank/01-clean.toml`), the error of any command (`error: ...`) or of a typed value that is no number (`error: 'loud' is not a number`), and for 10 seconds `Removed <id> — Undo` after a Module is removed.

Knobs: drag up to raise a value and down to lower it; 220 pixels cross the whole range. Hold Shift while dragging for steps ten times finer. Double-click a knob to reset the Parameter to its default. `preamp.stages` and `delay.pingpong` step in whole numbers. The sound changes while you drag: the GUI sends `set` at most every 30 ms and the final value on release. As with `xguitare set`, a change is not written to the Preset file until you save.

Values are shown with the unit, a whole value without decimals, others with one, a filter `q` with two: `−24 dB`, `5 ms`, `0.8 Hz`, `20 kHz`, `0.71`, a compressor ratio as `6 : 1`.

Typed values: every knob has a value field. Click it, or Ctrl+click or right-click the knob, and the field holds the bare number in the Parameter's unit (`20000` for `20 kHz`); type a value (`-8`, `12.5`, `12 dB`; `2 kHz` or `2k` is 2000) and press `Enter` to send it. `Esc` or a click elsewhere puts the shown value back; text that is no number does too, with a footer message. A value out of range is clamped by the Daemon, as with `xguitare set`.

Knobs from the keyboard: `Tab` moves to the next knob, which shows a ring, then to its value field. `↑` and `↓` move a focused knob's value by 1 % of the range (1 for a whole-number Parameter), `Page Up` and `Page Down` by 10 %, `Home` and `End` to the minimum and maximum. The `1` to `9` keys and Ctrl+S work while a knob has focus, but not while a value field has the text cursor; `Esc` releases the focus.

Rack editing, as with `xguitare rack`: in the left panel's `Rack` section, `Add Module` shows a tile per Module type. Click a tile to add that type where `Place` says: `by the Rack rules` puts a stereo Module at the end of the Rack, a mono Module or a cabinet after the last mono Module (at the front when there is none); choose `at the front` or `after <id>` to add it there instead. Or drag a tile into the rack: a line in the type's colour marks the gap, and releasing adds the Module there; released outside the rack it adds nothing. The Daemon refuses a place the Rack rules forbid. `↑` and `↓` move a Module one place, `Remove` in its open unit removes it; a Remove the Daemon refuses leaves the unit open. After a Remove, by the button or by dragging the unit out, the footer shows `Removed <id> — Undo` for 10 seconds, until another Preset is loaded: `Undo` adds the same Module type back after the Module that was above it (at its old position when that one is gone, at the front when it was first) and, once the Daemon has added it, sends its Parameter values, its bypass and its Cabinet IR as the GUI last read them. The Module comes back with the id the Daemon gives a new Module (its type, or `<type>-2` when that is taken), not a custom id from the Preset file. A layout the Daemon refuses (a stereo Module before the cabinet, say) leaves the Rack unchanged and shows the error in the footer. `Save` writes the new layout to the Preset file.

Cabinet IR: an open cabinet unit has an `IR` row below its knobs, a menu showing the cabinet's current Cabinet IR (`none` when it has none). Opening the menu reads the IR directory again, so it lists the same names as `xguitare ir list`, including files copied in since. Choosing a name loads that IR into the cabinet with a crossfade, as `xguitare ir set` does; an IR that cannot be loaded leaves the cabinet unchanged and shows the error in the footer. `Save` writes the new `ir` to the Preset file. The cabinet's `low_cut` and `high_cut` are Parameters, so they are knobs on the same unit.

Tuner: the Tuner unit's header shows the note and cents (`E1 −3 ct`) and a cents strip from −50 to +50 with a mark at 0 and a needle, or `no signal` without a confident reading, whether the unit is open or closed; its `Off`/`On` switch, like the `Tuner` button in the left panel, opens it to the Tuner view: the note name in large type (`no signal` without a confident reading), a half-circle scale from -50 cents (left) to +50 cents (right) with a needle, and the frequency (`41.2 Hz`) under it, green within 3 cents, amber beyond. When the config key `tuner_mute` is true (the default), opening the view turns Mute on, and pressing `Tuner` (or the unit's switch) again or closing the window turns it off. If Mute is already on when the view opens, the view leaves it alone, and closing the view leaves Mute on. Turning Mute off yourself with the `Mute` button means closing the view leaves Mute off. The `1` to `9` keys still load programs while the view is open.

Changes made elsewhere (the CLI, MIDI program change, an edited Preset file) show up at the next `status` poll, every 50 ms.

Keys: `1` to `9` load programs 0 to 8 (a key for a missing or invalid entry does nothing); `Ctrl+S` saves, like the `Save` button; `M` switches the Metronome on or off and `T` taps its tempo. Keys do nothing while a text field (search, Preset name, value, IR folder) has the text cursor. While a Project is open the keys are those of the Project view below.

When no Daemon answers on the socket, the GUI starts `xguitare run` itself, taken from the directory `xguitare-gui` is in, else from `PATH`, and shows `starting daemon` until it answers. The Daemon's log goes to the GUI's stderr. A Daemon the GUI started is stopped when the window closes; one you started yourself keeps running. If the GUI crashes or is killed, the Daemon it started keeps running and the sound does not stop; the next `xguitare-gui` connects to it. To stop that Daemon, send it SIGTERM: `kill <pid>`, with the pid from the GUI's `starting daemon ... (pid N)` log line or from `pgrep -x xguitare`. If the Daemon goes away, the left panel's `Engine` section shows `Daemon` `disconnected`, the footer `disconnected Ns`, and the GUI keeps reconnecting.

### Project view

While a Project is open the window becomes a small multitrack recorder; with none open it is the window described above.

- The transport bar across the top: `▶ Play` (lit green while playing or recording), `■ Stop`, `● Record` (lit red while recording, also during the count-in; clicked again it stops), during a count-in a red `count-in: N` with the beats left, `Mixer` and `Render` (see below), `Loop` and `Click` (lit while on), the count-in menu in bars (`no count-in`, `count-in 1`, `count-in 2`), the tempo (`120.0 bpm`: drag or type it; sent when the drag ends or on Enter) and `Meter` (type `3/4` and press Enter; anything else shows an error in the footer), then the position as `bar.beat.hundredths` and `m:ss.mmm`, all on one row. While `disk_overflows` is above 0 a red `disk too slow: frames dropped N times` follows.
- Below it the Tracks, one row each: a colour stripe, the name (type a new one and press Enter), the input menu (`no input` or a capture port, as `ports` lists them; opening it reads the list again), `R` arm (red), `M` mute, `S` solo, a meter of the Track's input, and volume (−60 to 6 dB) and pan (−1 to 1) as small knobs with a number field under each, sent when a drag ends or a value is typed; double-click a knob for 0 dB or centre. Click a row or its lane to select the Track; the selected row is outlined. `+ Add Track` adds one, as `xguitare track add` does.
- To the right the timeline: a ruler with bar numbers, the bar and beat grid, each Item (read with the Control API `items` whenever the Revision changes) as a block in its Track's colour labelled with its active Take's name (`guitar-2 (2/2)` when the Item has two Takes), the loop region shaded, and the playhead in red. Click the ruler to move the playhead there. Drag the loop region's start or end on the ruler to move it; it snaps to the beat and is sent as `loop` on release. Ctrl+wheel over the timeline zooms about the pointer, Shift+wheel or a sideways wheel scrolls it. The strip between the Tracks and the rack can be dragged to resize.
- Under it, headed `<Track> — Rack`, the rack shows the selected Track's Rack, edited as above; every change goes to that Track, and `Undo` after a Remove puts the Module back on the Track it was removed from, whichever Track is selected then. Opening, creating or closing a Project drops the Undo. Clicking a Bank row copies that Preset's Modules into the selected Track; while the mixer shows a Bus or Master Rack the Bank rows are greyed out (`a Preset loads only into a Track`).
- `Mixer` after `● Record` in the transport bar (or Ctrl+M) opens the mixer panel in place of the rack; clicked again it closes and the selected Track's Rack comes back. It has one column per Track, then one per Bus with `+ Add Bus` after the last, scrolled sideways when they do not fit, and Master at the right edge. A column shows its name, a pan knob (not on Master), a fader with its value in dB under it (+6 dB at the top; sent on release, double-click for 0 dB, or type a value; the lowest value sent is −60 dB, so a Master at `-inf` in `project.toml` shows −60), a stereo meter of the column's output with a 1 s peak hold, a clip LED above it (lit once `status` says clip; click it to reset that column's clip, as `xguitare clip reset` does) and `M` and `S` (not on Master). An `S` drawn dimmer-lit marks a column that sounds only through solo in place. A Track's column also has `to Master` and, per Bus, a Send knob with its level (`off` when turned all the way down) and a `pre` toggle; past two Buses the Sends scroll inside the column. The mixer opens with the selected Track's Rack below the columns; the strip between them can be dragged to resize, the window keeps that height, and the Rack scrolls inside the rest. Clicking a column's name opens its Rack below the columns instead, headed `<name> — Rack`: the Track's, the Bus's or the Master Rack, edited as above; on a Bus or Master Rack the `cabinet` tile is greyed out, with `a cabinet is not allowed on a Bus or Master` on hover. Right-click a Bus's name for `Remove`.
- `Render` after `Mixer` in the transport bar (or Ctrl+Alt+R) opens the Render dialog: `Range` `Project` or `Loop` (greyed out with no loop region), `Bits` `24`, `16` or `32f`, and `Name`, which shows the Project folder's name and may be changed. `Render` starts it (see [Render](#render)); a progress bar with `Cancel` follows it, and it ends with `rendered <file>`, the Daemon's error verbatim, or `render cancelled`. `Close`, Esc or a click outside closes the dialog; a running Render goes on.
- When `project_open` finds an autosave newer than `project.toml`, a prompt says `A newer autosave exists — <name>`: `Restore` closes the Project and opens it again from `project.autosave.toml`, `Open saved` keeps it as opened.

Keys in the Project view (Reaper's): `Space` play or stop, `Ctrl+R` record, `R` turn the loop on or off (with no loop region, the first 4 bars), `Home` move to the start, `End` to the end of the last Item or the loop region, `Ctrl+S` save the Project, `Ctrl+M` open or close the mixer panel, `Ctrl+Alt+R` open the Render dialog. `Ctrl+M` and `Ctrl+Alt+R` do nothing with no Project open. The `1` to `9` keys do not load Presets while a Project is open. `Space`, `R`, `Home` and `End` act only while no knob, button or field has focus (`Esc` releases it); `Ctrl+R`, `Ctrl+S`, `Ctrl+M` and `Ctrl+Alt+R` act unless a text field has the text cursor.

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

The Daemon keeps the choices made in the GUI (or through the Control API's `set_ports` and `set_settings`) in `state.toml` beside the config file, mode 0600, and never rewrites `config.toml`, so its comments survive. `state.toml` is read after `config.toml` and overrides it: `[ports]` (`input`, `output_left`, `output_right`, exact port names), `bank_kind`, `ir_dir`, `[banks]` (`guitar`, `bass`: each kind's Bank directory, recorded on the first switch; the one of `bank_kind` is the Bank directory), and `[metronome]` (`bpm`, `meter`, `volume_db`, see [Metronome](#metronome)); a `[metronome]` value out of range is dropped, and the Daemon logs `<path>: metronome: <error>; ignoring it`. A `bank_dir` in `config.toml` is never overridden, `state.toml`'s `bank_kind` is ignored while it is set, and the GUI cannot switch the Bank kind while it is set. If `state.toml` cannot be parsed, the Daemon logs `<path>: line N:M: <error>; ignoring it` and starts with `config.toml` alone. To drop the GUI's choices, stop the Daemon and delete `state.toml`.

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

The GUI bundles the IBM Plex fonts (Sans, Sans Condensed, Mono), licensed under the SIL Open Font License 1.1; the licence text is in `crates/xguitare-gui/assets/fonts/OFL.txt`.
