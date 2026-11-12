# Changelog

All notable changes to xguitare are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## Unreleased

### Added

- `autowah` Module, an envelope filter for bass and guitar: the input's envelope sweeps a low-, band- or high-pass filter between `freq_low` and `freq_high`, up or down, with `sensitivity`, `attack`, `decay`, `q`, `dry` and `wet`.
- `eq` Module, a parametric equaliser: 12 dB/oct `low_cut` and `high_cut`, a low and a high shelf, two bells with their own `q`, and `output`.
- `blend` Module, a bass DI blend drive: below `crossover` the signal stays clean, above it `presence` and `drive` clip it at `level`; `blend` mixes the clean input with that, and `bass` (40 or 80 Hz), `mid` (500 or 1000 Hz) and `treble` shape the result, also at `blend` 0.
- `limiter` Module, a peak limiter for the end of a Rack, a Bus or the Master: `input` drives it, the output never exceeds `ceiling`, `release` sets the recovery and `lookahead` (0–5 ms) delays the sound so a peak is caught before it passes; on a Bus or the Master both sides share one gain.
- `shaper` Module, a transient shaper: `attack` raises or lowers the start of each note and `sustain` what follows, with no threshold, plus `output` and `mix`; on a Bus or the Master both sides share one gain.
- Bass starter Presets Envelope Funk (compressor, `autowah`, `eq`, cabinet) and Modern Grind (`eq`, `blend`, `limiter`, cabinet); an existing bass Bank is not touched.
- GUI: the `eq` unit (`EQ`) shows its response curve, a small one when closed and, when open, a plot with a dot per band over four band columns of knobs.
- GUI: a choice Parameter is a row of buttons in an open unit and a chip with the current choice in a closed one (a click on a two-way chip switches it); an open unit grows to hold its knobs, which wrap to another row between groups of Parameters.
- Choice Parameters: the Control API `modules` and `get` entries carry `labels` (`["lp","bp","hp"]`; `[]` for a plain number) and `xguitare modules` prints them, `mode (lp|bp|hp)`. Presets and `set` keep the number.
- Metronome: a blip on every beat with beat 1 higher and 6 dB louder, at its own tempo (20–300 bpm), meter and volume (−40 to 0 dB) with no Project open, and as the Project's click with one open; tap tempo; Mute silences it while it keeps counting; tempo, meter and volume are kept in `state.toml`. CLI `metronome on|off|tempo|volume|tap`; Control API `metronome` and `metronome_tap`; `status` shows `metronome`. The GUI's `Metronome` section, with a beat light, and the `M` (on/off) and `T` (tap) keys.
- A GitHub Actions release workflow builds `xguitare` and `xguitare-gui` for x86_64 Linux on a `v*` tag and attaches a tarball and its sha256 to the GitHub Release.
- Render a Project to a stereo WAV in the background (`renders/<name>.wav`, never replacing a file): the whole Project with its tail, or the loop region; 24-bit, 16-bit with dither or 32-bit float. CLI `render [--loop] [--bits 16|24|32f] [--name <name>]` and `render cancel`; Control API `render` and `render_cancel`; `status` shows `render`.
- Mixer in a Project: up to 8 Buses with stereo Racks, a Send per Track and Bus (post- or pre-fader), a Master Rack and volume, `to master` per Track, solo in place, and output peaks with a latching clip flag per Track, Bus and Master. CLI `bus add|remove|edit`, `send`, `master`, `track edit --to-master`, `clip reset`, and `--bus <bus>` / `--master` on `rack`, `set`, `get`, `bypass` and `ir set`; Control API `bus_add`, `bus_remove`, `bus_set`, `send_set`, `master_set`, `clip_reset`, `track_set` `to_master`, and `bus` / `master` on the Rack requests; `status` shows `buses`, `master` and each Track's `to_master`, `sends`, `peak_db` and `clip`.
- Projects: record and play back up to 16 Tracks, each with its own Rack, input, volume, pan, mute, solo and arm, with tempo, meter, loop, click and count-in. CLI `project new|open|save|close|list`, `track add|remove|edit|select`, `play`, `stop`, `record`, `locate`, `tempo`, `click`; Control API `project_*`, `track_*`, `transport`, `loop`, `tempo`, `click`, and an optional `track` on the Rack requests. Takes are placed latency-compensated; an open Project is autosaved every 60 seconds while changed and when the Daemon stops.
- `status` shows `disk_overflows`, `count_in_remaining`, `loaded_audio_bytes`, `project_warning` and, while a Project is open, its transport, position, tempo, loop, click and Tracks; `settings` and `set_settings` take `projects_dir`.
- The Control API `items` request and CLI `project items` list each Track's Items; `track_select` selects a Track.
- The GUI turns into a Project view while a Project is open: transport bar, Track rows, a timeline with the Items, loop region and playhead, and the selected Track's Rack.
- First run creates 8 guitar and 8 bass starter Presets (Rock, Blues, Funk; bass Rock); existing Banks are not touched.
- `get` and `get <module>` end each Module's lines with `<id>.bypass = yes|no`.
- The Daemon follows a sample-rate change of the audio server: it rebuilds the Rack (Cabinet IRs resampled) and the Tuner at the new rate, logs `sample rate changed: 48000 -> 96000`, and `status` shows the new `sample_rate`.
- `get` on a cabinet ends with its Cabinet IR, `cabinet.ir = default-cab.wav` (`none` when it has none).
- The Daemon serves at most 32 connections at once; one more gets `too many connections` and is closed. A connection idle for 300 seconds is closed.
- Bypass per Module: `xguitare bypass <id> on|off` and the Control API `bypass` request switch one Module to passing its input through, with a 10 ms crossfade; a Preset's `[[module]]` takes `bypass = true`, `save` writes it, and `get` entries carry `bypass`.

### Changed

- GUI: new "Faceplate" look, part 1: one theme for every colour and text size, the IBM Plex fonts bundled in the binary, and a new set of seventeen Module colours, each distinct from the others and readable with dark text on it.
- GUI: "Faceplate" look, part 2: knobs with a printed tick ring and a value arc in the Module colour, ringed lamps, hover and pressed states on every button, a white focus ring on whichever control has the keyboard focus, text fields with a lit edge while editing, and a bypassed unit drawn in grey with a `Bypassed` tag instead of faded. A group of choice buttons is now one `Tab` stop.
- GUI: "Faceplate" look, part 3: each Rack unit is a faceplate between steel ears engraved with its Rack position and edged in its Module colour; the meter bridge has a dB scale; the Tuner unit has its own `Off`/`On` switch and a cents strip; the EQ plot sits in a recessed well beside its band columns, and a closed EQ lists its bands (`+4 @100 −3 @400`). `Remove` moved next to `↑` `↓` in an open unit.
- GUI: "Faceplate" look for the Project view: the transport bar and the Tracks are faceplates between steel ears, the position sits in a dark display in large digits, Play, Record, Loop, Click and the Track switches fill with their colour while on, each Track header has a colour tab and its number, and Items are dark plates with a Track-colour top edge; the Take being recorded is a dashed red plate growing to the playhead.
- GUI: with a Project open the main area shows one of three views, `Rack`, `Arrange` (Tracks and timeline over the Rack) or `Mixer`, chosen in the transport bar or with Ctrl+1, Ctrl+2, Ctrl+3; Ctrl+M shows the mixer and goes back to the view before it. The mixer no longer shares the window with a Rack: clicking a strip's name picks the Rack that `Rack` and `Arrange` show. The last view is kept across restarts.
- The Project click has an accent: beat 1 is 6 dB louder than the other beats, and the click plays at the Metronome volume (default −12 dB), quieter than before.
- Cabinet IRs are normalised to unit energy on load, so Banks created before this change sound about 12 dB quieter. Raise the cabinet `level` in your Presets (about 12, maximum 20); the starter Presets carry `level = 12`.
- A stereo Cabinet IR is normalised over both channels, so it plays as loud in total as a mono IR; a right-only IR is no longer boosted.
- Only the first `ir_max_ms` of a Cabinet IR is read.
- `set` rounds the value of an integer Parameter (`preamp.stages`, `delay.pingpong`, `cabinet.low_cut`, `cabinet.high_cut`) before clamping, so `set cabinet.low_cut 150.4` answers and saves 150.
- Mute fades over 10 ms at any sample rate.
- A Preset switch that arrives during a crossfade waits for it to end and then crossfades; of several, only the newest runs.
- `status` peaks are kept per connection, so the GUI and the CLI no longer reset each other's.
- `save` keeps the Preset file's permissions and writes through a symlink to its target.
- The Daemon keeps running when a GUI that started it crashes; closing the window cleanly still stops it.
- A second `xguitare run` on the same socket exits with `error: another Daemon is running` (it was `error: daemon already running at <socket>`); the running Daemon holds a lock on `<socket>.lock`, e.g. `xguitare.sock.lock`.
- `xguitare rack` with no subcommand prints one `error:` line and exits 1.

### Fixed

- A Cabinet IR with a NaN or infinite sample is refused with `ir <name>: non-finite sample`.
- `rack_add cabinet` finds `default-cab.wav` or `bass-cab.wav` under the IR directory as well as the Bank directory.
- An IR path error names both directories it looked in.
- `crossfade_ms` (above 0 to 1000) and `ir_max_ms` (above 0 to 10000) out of range, NaN or infinite are refused when the config loads.
- A preamp `stages` change no longer jumps in level.
- The CLI and the GUI refuse a fallback socket directory that is not yours or not mode 0700.

### Known

- Some crates under eframe/winit are unmaintained: ttf-parser (RUSTSEC-2026-0192) and zbus_names, zvariant, zvariant_derive (RUSTSEC-2026-0370, RUSTSEC-2026-0371, RUSTSEC-2026-0372). They are transitive; nothing to do until egui moves off them.
