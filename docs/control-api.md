# Control API

The protocol a client uses to inspect and change the running Daemon: which Preset is active, the Bank, Parameter values and the Rack's layout. The `xguitare` CLI is one client of it; a GUI or script can be another. Request and response types are in the crate `xguitare-protocol` (`crates/xguitare-protocol/src/lib.rs`, see ADR-0006); the Daemon answers requests in `crates/xguitare-core/src/control.rs`, and the socket server is `crates/xguitare/src/socket_server.rs`.

## Transport

- A Unix stream socket at the config key `socket`, by default `$XDG_RUNTIME_DIR/xguitare/xguitare.sock` (`/tmp/xguitare-<uid>/xguitare.sock` when `$XDG_RUNTIME_DIR` is unset).
- Each request is one JSON object on one line, ended by `\n`. Each request gets exactly one response: one JSON object on one line, ended by `\n`. Requests on one connection are answered in order.
- The connection stays open after a response; send the next request on it, or close it. The CLI opens one connection per command.
- A line that is not a valid request (malformed JSON, an unknown `cmd`, a missing or wrongly typed field, an empty line) gets an error response starting with `invalid request: ` followed by the JSON parser's message, and the connection stays open.
- A line longer than 64 KiB gets `{"ok":false,"error":"request too long"}` and the Daemon closes the connection.
- The Daemon serves at most 32 connections at once. One more gets `{"ok":false,"error":"too many connections"}`, without sending a request, and is closed.
- A connection that sends nothing for 300 seconds is closed by the Daemon; a client that keeps one open reconnects.
- One Daemon per socket: the Daemon holds an exclusive `flock` on `<socket>.lock` (e.g. `xguitare.sock.lock`) while it runs, and a second Daemon on the same socket exits with `error: another Daemon is running`.

Every response has a boolean `ok`. On success it carries the request's payload keys next to `"ok":true`; on failure it is `{"ok":false,"error":"<message>"}`, and an error a client may act on also carries a machine-readable `code`, e.g. `{"ok":false,"error":"a project is still closing; try again","code":"still_closing"}`. Key order in responses is not significant.

| `code` | Error |
| --- | --- |
| `still_closing` | `a project is still closing; try again` from `project_new` and `project_open`. |

A client ignores a `code` and a reply key it does not know, and treats an error without a `code` by its `error` text, as a Daemon from before codes sends none.

### Versioning

`status` carries `protocol_version`, now `1`. It goes up when a request or reply field is removed or changes meaning; an addition (a new request, reply key or `code`) leaves it as it is, because an old client ignores what it does not know and a new client handles a reply without it. A `status` without `protocol_version` comes from a Daemon from before versioning, which speaks version 1. The CLI's `status` prints a `warning:` line, and the GUI a notice, when the Daemon's version is higher than their own.

Errors any request can return:

| `error` | When |
| --- | --- |
| `invalid request: <parser message>` | The line is not a valid request, e.g. `invalid request: expected ident at line 1 column 2` for `not json`, ``invalid request: unknown variant `reboot`, expected one of `status`, `list`, `load`, `set`, `get`, `save`, `modules`, `rack_add`, `rack_remove`, `rack_move`, `tuner`, `mute`, `bypass`, `ir_list`, `rack_ir`, `ports`, `set_ports`, `settings`, `set_settings`, `project_new`, `project_open`, `project_save`, `project_close`, `project_list`, `project_undo`, `project_redo`, `items`, `track_add`, `track_remove`, `track_set`, `track_select`, `transport`, `loop`, `tempo`, `click`, `metronome`, `metronome_tap`, `bus_add`, `bus_remove`, `bus_set`, `send_set`, `master_set`, `render`, `render_cancel`, `clip_reset`, `item_move`, `item_trim`, `item_split`, `item_delete`, `item_take`, `item_duplicate`, `item_copy`, `item_paste` at line 1 column 15``, ``invalid request: missing field `preset` ``. |
| `request too long` | The line exceeds 64 KiB; the connection is then closed. |
| `too many connections` | Sent unasked on connect when 32 connections are already open; the connection is then closed. |

Parameter addresses are `<module id>.<param name>`, e.g. `preamp.gain`. A Module's id is its `id` in the Preset file, or its type when the file gives none.

### Track scope

While a Project is open the Engine plays its Tracks, not the active Preset's Rack. `load`, `set`, `get`, `bypass`, `rack_add`, `rack_remove`, `rack_move` and `rack_ir` then act on one Track's Modules: the one named by an optional `track` key, else the selected Track (see `track_select`). `track` is a zero-based index (a JSON number) or a Track name (a JSON string). `load` copies the Preset's Modules into the Track. The active Preset, `modified` and `save` stay as they were; the Track's changes are saved with the Project.

```json
{"cmd":"set","param":"preamp.gain","value":20,"track":"Bass"}
```

In place of `track`, `set`, `get`, `bypass`, `rack_add`, `rack_remove`, `rack_move` and `rack_ir` take `bus` (a zero-based index or a Bus name, see `bus_add`) to act on that Bus's Rack, or `"master":true` to act on the Master Rack. These are stereo Racks: they take any Module but the cabinet, in any order. `load` takes them too, but only to refuse them.

```json
{"cmd":"rack_add","type":"reverb","bus":"Verb"}
{"cmd":"set","param":"compressor.threshold","value":-18,"master":true}
```

Errors of the Track scope, on any of these requests:

| `error` | When |
| --- | --- |
| `no project open` | `track`, `bus` or `master` is given while no Project is open. |
| `the project has no tracks` | A Project is open, `track`, `bus` and `master` are left out and the Project has no Track. |
| `no track '<track>'` | No Track has that index or name, e.g. `no track '5'`. |
| `no bus '<bus>'` | No Bus has that index or name, e.g. `no bus 'Echo'`. |
| `give track, bus or master, not more than one` | More than one of `track`, `bus` and `"master":true` is given. |
| `a cabinet is not allowed on a Bus or Master` | `rack_add` of a `cabinet`, or `rack_ir`, aimed at a Bus or Master. |
| `a Preset loads only into a Track` | `load` with `bus` or `master`. |

## status

Request:

```json
{"cmd":"status"}
```

Response:

```json
{"ok":true,"buffer_size":128,"callback_last_us":926,"callback_max_us":1484,"click":{"count_in":1,"on":true},"connections":[{"from":"Scarlett Solo 4th Gen Input 1 Inst/Line:capture_MONO","to":"xguitare:in"},{"from":"xguitare:out_l","to":"Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FL"},{"from":"xguitare:out_r","to":"Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FR"}],"count_in_remaining":0,"disk_overflows":0,"input_peak_db":-96.1,"loaded_audio_bytes":3840000,"loop":{"end":172800,"on":true,"start":0},"metronome":{"beat":2,"bpm":100.0,"meter":[3,4],"on":true,"volume_db":-12.0},"modified":false,"muted":false,"output_peak_db":-120.0,"position":{"bars_beats":"1.1","frames":0},"prepared_frames":128,"preset":"Clean","program":0,"project":"Demo Song","project_warning":null,"protocol_version":1,"render":{"file":"/home/you/Music/xguitare/demo-song/renders/demo-song.wav","name":"demo-song","progress":1.0,"range":"project","state":"done"},"revision":10,"sample_rate":48000,"selected":1,"undo":"move 2 Items","redo":null,"clipboard":0,"tempo":{"bpm":100.0,"meter":[3,4]},"tracks":[{"armed":true,"clip":false,"id":0,"input":"Scarlett Solo 4th Gen Input 1 Inst/Line:capture_MONO","input_peak_db":-120.0,"mute":false,"name":"Track 1","pan":-0.25,"peak_db":[-14.2,-17.9],"sends":[{"bus":0,"level_db":-6.0,"pre":false}],"solo":false,"to_master":true,"volume_db":-3.0},{"armed":false,"clip":false,"id":1,"input":null,"input_peak_db":-120.0,"mute":false,"name":"Bass","pan":0.0,"peak_db":[-120.0,-120.0],"sends":[],"solo":false,"to_master":true,"volume_db":0.0}],"buses":[{"clip":false,"id":0,"mute":false,"name":"Verb","pan":0.0,"peak_db":[-30.1,-29.8],"solo":false,"volume_db":0.0}],"master":{"clip":false,"peak_db":[-13.9,-16.8],"volume_db":0.0},"transport":"stopped","xruns":0}
```

| Key | Value |
| --- | --- |
| `preset` | Name of the active Preset; `null` when none is loaded (empty Bank, or the first Preset failed to load at start). |
| `program` | Bank position of the active Preset's file; `null` when no Preset is loaded or its file has been removed from the Bank. The `preset` name stays while the removed Preset keeps playing. |
| `sample_rate` | JACK sample rate in Hz; follows a rate change while the Daemon runs, and the Rack and the Tuner are rebuilt at the new rate. An open Project stops playing while the rate differs from its own (see `project_warning`) and plays again once they match. |
| `buffer_size` | JACK buffer size in frames, as currently granted. |
| `prepared_frames` | Block size in frames the running Rack was prepared for. The Daemon re-prepares the Rack, and an open Project's Tracks, within a second when `buffer_size` differs. |
| `revision` | The Revision: starts at 1 and increments on every successful `load`, `set`, `bypass`, `save`, `rack_add`, `rack_remove`, `rack_move` and `rack_ir`, on a hot reload of the active Preset, on a Bank re-index and on a re-prepare; with Projects also on `project_new`, `project_open`, `project_close`, `project_undo`, `project_redo`, `track_add`, `track_remove`, `track_set`, `track_select`, `loop`, `tempo`, `click`, `bus_add`, `bus_remove`, `bus_set`, `send_set`, `master_set`, `metronome`, `metronome_tap` (from its second tap), a Rack request on a Track, Bus or Master, each Take added to the open Project, and when a Render starts (`render`) and when it ends. A client refreshes when it changes. `mute` does not change it. |
| `modified` | Whether the live Rack has been changed since the active Preset was last loaded, reloaded or saved: `true` after a successful `set`, `bypass`, `rack_add`, `rack_remove`, `rack_move` or `rack_ir`; `false` after a `load`, a `save` (with or without `as`) or a hot reload of the active Preset, and when no Preset is loaded. |
| `muted` | Whether Mute is on (see `mute`). |
| `input_peak_db` | Largest input sample since the previous `status` on the same connection (for the first `status` on a connection, since the previous `status` on any connection), in dB (`20·log10`), one decimal; `-120.0` for silence. Each `status` resets it for its own connection only. |
| `output_peak_db` | As `input_peak_db`, over both outputs. |
| `xruns` | Xruns since the Daemon started. |
| `callback_max_us` | Longest audio callback so far, in µs. |
| `callback_last_us` | Most recent audio callback, in µs. |
| `connections` | Port connections the Daemon made at start, each `{"from": <port>, "to": <port>}`. |
| `metronome` | The Metronome (see `metronome`): whether it is `on`, `bpm`, `meter` `[<beats per bar>, <note value>]`, `volume_db`, and `beat`, the beat in the bar it last played, counted from 1 (`0` whenever `on` is `false`; it counts on under Mute). `beats_played` counts the beats started so far (a u64 that wraps), so a client sees a new beat even when `beat` repeats, as with a one-beat meter. While a Project is open, `on`, `bpm` and `meter` are the Project's click, tempo and meter, and `beat` counts the click; a count-in with the click off still sounds, but `beat` stays `0`. |
| `disk_overflows` | Times since the Daemon started that a Track's recording ring was full and frames were dropped because the disk fell behind. |
| `count_in_remaining` | Beats left in the count-in before recording starts, rounded up; `0` when no count-in is under way or no Project is open. |
| `loaded_audio_bytes` | Bytes of Take audio the open Project holds in memory (4 per frame); `0` when no Project is open. |
| `project_warning` | Why the open Project does not play, e.g. `project rate 48000 Hz differs from the engine rate 44100 Hz` after a sample-rate change: the transport stops, Takes still open are finished, and `play` and `record` are refused until the rates match; `null` otherwise. |
| `render` | The running Render, or the last one until the next starts (also after its Project is closed, until another Project is opened or created); `null` before the first and after `project_new` or `project_open`. `name`, `range` (`project` or `loop`), `progress` from 0 to 1 (the tail after the last Item does not count), `state` (`running`, `done`, `cancelled` or `failed`), `file` (the WAV it writes or wrote) and, when it failed, `error`. |
| `protocol_version` | The Control API version the Daemon speaks (see Versioning). |
| `project` | Name of the open Project; `null` when none is open. The keys below are `null` (`tracks` and `buses` empty) while no Project is open. |
| `transport` | `stopped`, `playing` or `recording` (also during the count-in), as the audio thread runs it. |
| `position` | The playhead: `frames` from the Project start, counted from 0, and `bars_beats` as `<bar>.<beat>`, counted from 1 at the Project's tempo and meter. |
| `tempo` | `bpm` and `meter` `[<beats per bar>, <note value>]`. |
| `loop` | The loop region: `start` and `end` in frames, and whether it is `on`. |
| `click` | Whether the click is `on`, and `count_in` bars before recording. |
| `selected` | Index of the selected Track. |
| `undo`, `redo` | The label of what `project_undo` and `project_redo` would take back or do again, e.g. `move 2 Items` or `record Bass DI`; `null` when there is none. |
| `clipboard` | How many Items `item_copy` put in the clipboard, which `item_paste` places; `0` when it is empty. |
| `tracks` | One entry per Track in order: `id` (as in `items`), `name`, `input` (capture port or `null`), `volume_db`, `pan`, `mute`, `solo`, `armed`, `input_peak_db` of its input port, measured as `input_peak_db`, `to_master`, `sends`, `peak_db` and `clip`. `sends` lists the Track's Sends that are not off, each `{"bus": <Bus index>, "level_db": <dB>, "pre": <bool>}`. `peak_db` is `[left, right]`, the Track's output after its volume and pan, measured as `input_peak_db` (one decimal, `-120.0` for silence, reset per connection by each `status`). `clip` is `true` once a side has reached 0 dBFS, until `clip_reset`. |
| `buses` | One entry per Bus in order: `id` (stays the Bus's while others are added or removed, and is not given to a later Bus while the Project is open; `project.toml` names it in each Send), `name`, `volume_db`, `pan`, `mute`, `solo`, and `peak_db` and `clip` as for a Track, after the Bus's Rack, volume and pan. |
| `master` | `volume_db` (`null` when off), and `peak_db` and `clip` as for a Track, after the Master Rack and volume, before the click. |

Errors: none besides the transport errors.

## list

Request:

```json
{"cmd":"list"}
```

Response:

```json
{"ok":true,"active":0,"entries":[{"name":"Clean","program":0,"valid":true},{"name":"Crunch","program":1,"valid":true},{"name":"Lead","program":2,"valid":true},{"name":"High Gain","program":3,"valid":true},{"name":"Bass","program":4,"valid":true},{"error":"line 1:8: string values must be quoted, expected literal string","name":"06-broken","program":5,"valid":false}]}
```

| Key | Value |
| --- | --- |
| `active` | Program number of the active Preset, `null` as for `status.program`. |
| `entries` | Every `*.toml` file of the Bank directory, sorted by file name. |
| `entries[].program` | Position in the Bank, from 0; the number `load` and MIDI program change use. |
| `entries[].name` | The Preset's `name`; for an invalid file, the file name without `.toml`. |
| `entries[].valid` | `false` when the file does not parse as a Preset. |
| `entries[].error` | Present only when `valid` is `false`: why the file does not parse. |

Errors: none besides the transport errors.

## load

Switches the running Rack to a Bank entry, with a crossfade. A switch that arrives while a crossfade runs waits for it to end and then crossfades; when several arrive, only the newest runs. `preset` is a program number (all digits) or a Preset name (exact match, first in Bank order).

Request:

```json
{"cmd":"load","preset":"Crunch"}
```

Response:

```json
{"ok":true,"name":"Crunch","program":1}
```

| Key | Value |
| --- | --- |
| `program` | Program number of the loaded Preset. |
| `name` | Its name. |

Errors:

| `error` | When |
| --- | --- |
| `no preset '<preset>'` | No entry has that number or name, e.g. `no preset 'Nope'`, `no preset '9'`. |
| the entry's parse error | The entry is invalid; the error is its `list` `error` text, e.g. `line 1:8: string values must be quoted, expected literal string`. |
| `engine busy` | The audio thread's command queue is full; retry. |

## set

Changes one Parameter of the running Rack. The value is rounded to a whole number for an integer Parameter, then clamped to the Parameter's range. The change is not written to the Preset file until `save`.

Request:

```json
{"cmd":"set","param":"preamp.gain","value":150}
```

Response:

```json
{"ok":true,"clamped":true,"param":"preamp.gain","value":100.0}
```

| Key | Value |
| --- | --- |
| `param` | The address as given. |
| `value` | The value applied, after rounding and clamping. |
| `clamped` | `true` when the requested value was outside the range. |

Errors:

| `error` | When |
| --- | --- |
| `expected <module>.<param>, got '<param>'` | `param` is not `<module>.<param>`, e.g. `expected <module>.<param>, got 'preamp'`. |
| `no module '<module>'` | The active Preset has no Module with that id. |
| `module '<module>' has no parameter '<param>'` | The Module has no such Parameter. |
| `no preset loaded` | No Preset is active. |
| `engine busy` | The audio thread's command queue is full; retry. |
| `invalid request: invalid type: string "loud", expected f64` | `value` is not a JSON number (transport error). |

## get

Reads Parameter values of the active Preset as they are now, including unsaved `set` changes. `param` is optional: omitted for every Parameter, a Module id for that Module's, or `<module>.<param>` for one.

Request:

```json
{"cmd":"get","param":"preamp.gain"}
```

Response:

```json
{"ok":true,"params":[{"bypass":false,"default":40.0,"integer":false,"labels":[],"max":100.0,"min":0.0,"module":"preamp","param":"gain","type":"preamp","unit":"%","value":100.0}]}
```

| Key | Value |
| --- | --- |
| `params` | The selected Parameters in Rack order, then Parameter order. |
| `params[].module` | Module id. |
| `params[].type` | The Module's type, as in `modules`. |
| `params[].param` | Parameter name. |
| `params[].value` | Current value. |
| `params[].unit` | `"dB"`, `"ms"`, `"Hz"`, `"%"`, or `""` for a plain number. |
| `params[].min`, `params[].max` | The Parameter's range. |
| `params[].default` | The value a new Module of this type starts with. |
| `params[].integer` | `true` when the Parameter takes whole numbers only (`preamp.stages`, `delay.pingpong`, `cabinet.low_cut`, `cabinet.high_cut`, and every choice Parameter, such as `autowah.mode`); `set` and the Module round other values. |
| `params[].labels` | The names of a choice Parameter's values, `["lp","bp","hp"]` for `autowah.mode`: value `n` means `labels[n]`, from 0 to `max`. `[]` for a plain number. `set` and Presets take the number. |
| `params[].bypass` | Whether the Module is bypassed (see `bypass`); the same on every entry of one Module. |
| `params[].ir` | Cabinet entries only: the Module's Cabinet IR name, as in the Preset's `ir` (`null` when it has none). Other Modules' entries have no `ir` key. |

Errors:

| `error` | When |
| --- | --- |
| `no module '<module>'` | No Module with that id, e.g. `no module 'amp'`. |
| `module '<module>' has no parameter '<param>'` | The Module has no such Parameter. |
| `expected <module>.<param>, got '<param>'` | `param` contains a `.` but is not a valid address, e.g. `preamp.`. |
| `no preset loaded` | No Preset is active. |

## save

Writes the active Preset, with its current Parameter values, back to its file in the Bank directory (written to `<file>.tmp`, then renamed over the file).

Request:

```json
{"cmd":"save"}
```

Response:

```json
{"ok":true,"path":"/home/you/.config/xguitare/bank/03-lead.toml"}
```

| Key | Value |
| --- | --- |
| `path` | The file written. |

With `as`, writes the active Preset, with its current Parameter values and the given name (control characters U+0000 to U+001F and U+007F removed, then leading and trailing whitespace), to a new file in the Bank directory instead, and makes that the active Preset; the Bank is read again. An existing file is never replaced: the new file is a hard link to a temp file written beside it, or, on a filesystem without hard links, is created and written directly. The file is `<NN>-<slug>.toml`: `NN` is one more than the highest number before the first `-` in a Bank file name, at least two digits; `slug` is the name lowercased with ASCII letters and digits kept and every other run of characters replaced by one `-`, without a `-` at either end. The name reaches the file name only through the slug, so the file is always directly in the Bank directory.

Request:

```json
{"cmd":"save","as":"My Lead Tone!"}
```

Response:

```json
{"ok":true,"path":"/home/you/.config/xguitare/bank/04-my-lead-tone.toml","program":3}
```

| Key | Value |
| --- | --- |
| `path` | The new file. |
| `program` | Bank position of the new file, now the active Preset. |

Errors:

| `error` | When |
| --- | --- |
| `no preset loaded` | No Preset is active. |
| `active preset file was removed; load a preset first` | Without `as`: the active Preset's file is no longer in the Bank. |
| `preset name must not be empty` | With `as`: the name is empty or has no ASCII letter or digit. |
| `a preset named '<name>' already exists` | With `as`: a Preset in the Bank has this name, compared case-insensitively. |
| `cannot save <path>: file exists` | With `as`: files appeared at the new file's name and the seven numbers after it while saving. |
| `cannot save <path>: <reason>` | Writing or renaming the file, or with `as` reading the Bank directory again, failed. |

## modules

Lists the Module types a Preset can use, in registry order.

Request:

```json
{"cmd":"modules"}
```

Response (shortened to two types):

```json
{"ok":true,"modules":[{"channels":"mono","params":[{"default":0.0,"integer":false,"labels":[],"max":20.0,"min":-60.0,"name":"level","unit":"dB"}],"type":"gain"},{"channels":"mono_to_stereo","params":[{"default":0.0,"integer":false,"labels":[],"max":20.0,"min":-20.0,"name":"level","unit":"dB"},{"default":20.0,"integer":true,"labels":[],"max":300.0,"min":20.0,"name":"low_cut","unit":"Hz"},{"default":20000.0,"integer":true,"labels":[],"max":20000.0,"min":2000.0,"name":"high_cut","unit":"Hz"}],"type":"cabinet"}]}
```

| Key | Value |
| --- | --- |
| `modules[].type` | The Module type, as written in a Preset's `type`. |
| `modules[].channels` | `"mono"` (before the cabinet), `"stereo"` (after it) or `"mono_to_stereo"` (the cabinet). |
| `modules[].params` | The Parameters in order, each with `name`, `min`, `max`, `default`, `unit`, `integer` and `labels` (as in `get`). |

Errors: none besides the transport errors.

## rack_add

Inserts a Module with default Parameters into the active Preset, rebuilds the Rack from it and swaps it in with a crossfade. `after` is the id of the Module to insert after, or `at` the zero-based position to insert at (past the end means at the end); give at most one of them. With neither (or both `null`), the Module goes at the end. The new Module's id is its type, or `<type>-2`, `<type>-3`… when that id is taken. A cabinet gets `ir = "default-cab.wav"`, or `"bass-cab.wav"` when only that file is found; each name is looked up under the IR directory (`ir_dir`) and then the Bank directory, as a Preset's `ir` is. The change is not written to the Preset file until `save`.

Request:

```json
{"cmd":"rack_add","type":"delay","after":"reverb"}
```

Response:

```json
{"ok":true,"id":"delay"}
```

| Key | Value |
| --- | --- |
| `id` | The new Module's id. |

Errors (on any of them nothing changes):

| `error` | When |
| --- | --- |
| `unknown module type '<type>'` | No Module type has that name, e.g. `unknown module type 'wah'`. |
| `give either after or at` | The request has both `after` and `at`. |
| `no module '<after>'` | The active Preset has no Module with id `after`. |
| `stereo module '<id>' before the cabinet` | A stereo Module would come before the cabinet, or the Rack has no cabinet. |
| `mono module '<id>' after the cabinet` | A mono Module would come after the cabinet. |
| `more than one cabinet` | The Rack already has a cabinet. |
| `cabinet needs an ir; add one in the preset file` | A cabinet is added where the layout allows one, and neither `default-cab.wav` nor `bass-cab.wav` is in the IR directory or the Bank directory. |
| `no preset loaded` | No Preset is active. |
| `engine busy` | The audio thread's command queue is full; retry. |

## rack_remove

Removes a Module from the active Preset and swaps in the rebuilt Rack, as `rack_add` does.

Request:

```json
{"cmd":"rack_remove","id":"delay"}
```

Response:

```json
{"ok":true,"id":"delay"}
```

| Key | Value |
| --- | --- |
| `id` | The removed Module's id. |

Errors: `no module '<id>'`, `no preset loaded`, `engine busy`, and the layout errors of `rack_add` (removing the cabinet while stereo Modules follow it gives `stereo module '<id>' before the cabinet`); on any of them nothing changes.

## rack_move

Moves a Module of the active Preset to the zero-based position `to` and swaps in the rebuilt Rack, as `rack_add` does. A `to` past the end moves it to the end.

Request:

```json
{"cmd":"rack_move","id":"delay","to":6}
```

Response:

```json
{"ok":true,"id":"delay","to":5}
```

| Key | Value |
| --- | --- |
| `id` | The moved Module's id. |
| `to` | Its position now. |

Errors: `no module '<id>'`, `no preset loaded`, `engine busy`, and the layout errors of `rack_add`; on any of them nothing changes.

## ir_list

Lists the Cabinet IRs in the IR directory (config key `ir_dir`, default the Bank directory): every regular file whose extension is `.wav` in any case, in all subdirectories. Names starting with `.` or not valid UTF-8 are skipped, files and directories alike, and so is a subdirectory that cannot be read. A symlink is followed, except one inside a directory that was itself reached through a symlink. The directory is read on every request.

Request:

```json
{"cmd":"ir_list"}
```

Response:

```json
{"ok":true,"ir_dir":"/home/you/.config/xguitare/bank","irs":["Loud.WAV","bass-cab.wav","celestion/v30.wav"]}
```

| Key | Value |
| --- | --- |
| `ir_dir` | The IR directory listed. |
| `irs` | Each Cabinet IR's path relative to `ir_dir`, `/`-separated, sorted bytewise (upper case before lower case); the name `rack_ir` and a Preset's `ir` take. Empty when there is none. |

Errors:

| `error` | When |
| --- | --- |
| `ir_dir <path>: <reason>` | The IR directory cannot be read, e.g. `ir_dir /srv/irs: No such file or directory (os error 2)`. |

## rack_ir

Sets the Cabinet IR of a cabinet Module in the active Preset, rebuilds the Rack from it and swaps it in with a crossfade, as `rack_add` does. `ir` is looked up as `<ir_dir>/<ir>` and, when that file does not exist, as `<Bank directory>/<ir>`. `id` is optional: omitted, the first cabinet Module is used. Unlike `load`, which runs a cabinet whose IR fails to load as a passthrough and warns, `rack_ir` fails and changes nothing. The change is not written to the Preset file until `save`.

Request:

```json
{"cmd":"rack_ir","ir":"celestion/v30.wav","id":"cabinet"}
```

Response:

```json
{"ok":true,"id":"cabinet","ir":"celestion/v30.wav"}
```

| Key | Value |
| --- | --- |
| `id` | The cabinet Module's id. |
| `ir` | Its Cabinet IR name now. |

Errors (on any of them nothing changes and the Revision stays):

| `error` | When |
| --- | --- |
| `no cabinet module` | `id` is omitted and the active Preset has no cabinet. |
| `no module '<id>'` | The active Preset has no Module with that id. |
| `module '<id>' is not a cabinet` | The Module with that id is not a cabinet, e.g. `module 'gate' is not a cabinet`. |
| `invalid ir name '<ir>'` | `ir` is empty, absolute, or has a `.`, `..` or empty path segment, e.g. `invalid ir name '../x.wav'`. |
| `<path>: <reason>` | The Cabinet IR cannot be loaded, e.g. `/srv/irs/bad.wav: unsupported wav format: 8-bit int`; `<path>` is the `ir_dir` path when the file is in neither directory. |
| `ir <name>: non-finite sample` | The Cabinet IR has a NaN or infinite sample; `<name>` is the `ir` as given, e.g. `ir bad.wav: non-finite sample`. |
| `no preset loaded` | No Preset is active. |
| `engine busy` | The audio thread's command queue is full; retry. |

## tuner

The Tuner's last reading of the dry input. The Daemon analyses the input ten times a second; this request returns the latest result and never analyses itself.

Request:

```json
{"cmd":"tuner"}
```

Response:

```json
{"ok":true,"cents":0,"confident":true,"hz":110.0,"note":"A2"}
```

A pitch detected, but not clearly enough to show:

```json
{"ok":true,"cents":12,"confident":false,"hz":83.0,"note":"E2"}
```

With no pitch detected (silence or input below -60 dBFS, or a pitch outside 35 to 1400 Hz):

```json
{"ok":true,"cents":null,"confident":false,"hz":null,"note":null}
```

| Key | Value |
| --- | --- |
| `hz` | Detected frequency in Hz, one decimal; `null` when nothing is detected. |
| `note` | The nearest note and its octave, sharps only, A4 at the config key `tuner_reference_hz` (default 440), e.g. `E1`, `A#4`; `null` when nothing is detected. |
| `cents` | Distance from `note` in cents, an integer from -49 to 50; `null` when nothing is detected. |
| `confident` | Whether the reading is clear enough to show. Clients (the CLI and the GUI) show only confident readings and treat an unconfident one like no pitch. |

Errors: none besides the transport errors.

## mute

Turns Mute on or off: while on, both outputs fade to silence over 10 ms at any sample rate and stay silent, while the Tuner still reads the input. Off fades them back in. Mute stays on until a client turns it off, also when the client that turned it on disconnects. The Revision does not change.

Request:

```json
{"cmd":"mute","on":true}
```

Response:

```json
{"ok":true,"muted":true}
```

| Key | Value |
| --- | --- |
| `muted` | The Mute state now. |

Errors: `on` missing or not a boolean gets the transport's `invalid request: ` error, e.g. ``invalid request: missing field `on` ``; Mute is unchanged.

## bypass

Bypasses one Module of the running Rack (`on: true`) or switches it back in (`on: false`). A bypassed Module passes its input through unchanged and keeps its Parameters; the switch crossfades over 10 ms between the Module's output and its input. Once the fade to bypassed has ended the Module is not processed; switched back in, it starts from silence (delay and reverb tails restart). The change is not written to the Preset file until `save`, which writes `bypass = true` on the Module. The Revision increments and `status` shows `modified: true`.

Request:

```json
{"cmd":"bypass","id":"preamp","on":true}
```

Response:

```json
{"ok":true,"bypass":true}
```

| Key | Value |
| --- | --- |
| `bypass` | Whether the Module is bypassed now. |

Errors (on any of them nothing changes and the Revision stays):

| `error` | When |
| --- | --- |
| `no module '<id>'` | The active Preset has no Module with that id, e.g. `no module 'wah'`. |
| `no preset loaded` | No Preset is active. |
| `engine busy` | The audio thread's command queue is full; retry. |
| ``invalid request: missing field `on` `` | `on` is missing or not a boolean (transport error). |

## ports

Lists the JACK audio ports of other clients, physical or not (devices, PipeWire split nodes, other apps), and which of them the Daemon's ports are linked to. The ports are read on every request.

Request:

```json
{"cmd":"ports"}
```

Response:

```json
{"ok":true,"capture":["Scarlett Solo 4th Gen Input 1 Inst/Line:capture_MONO","Scarlett Solo 4th Gen Input 1 Inst/Line:capture_FL"],"playback":["Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FL","Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FR"],"connected":{"input":"Scarlett Solo 4th Gen Input 1 Inst/Line:capture_MONO","output_left":"Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FL","output_right":null}}
```

| Key | Value |
| --- | --- |
| `capture` | Ports the Daemon's input can take, sorted bytewise. |
| `playback` | Ports the Daemon's outputs can feed, sorted bytewise. |
| `connected` | `input`: the capture port linked to the Daemon's `in`; `output_left`, `output_right`: the playback ports `out_l` and `out_r` feed; each `null` when the Daemon made no such link. Links made by other tools are not listed. |

## set_ports

Links the Daemon's three ports to the named ports now: links the Daemon made that differ are dropped, the new ones made, ones already in place kept. The names are stored in `state.toml` (see the README's Config section), so the next start connects them again; `config.toml` is not changed. The Revision increments.

Request:

```json
{"cmd":"set_ports","input":"Scarlett Solo 4th Gen Input 1 Inst/Line:capture_MONO","output_left":"Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FL","output_right":"Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FR"}
```

Response: `connected` as in `ports`.

```json
{"ok":true,"connected":{"input":"Scarlett Solo 4th Gen Input 1 Inst/Line:capture_MONO","output_left":"Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FL","output_right":"Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FR"}}
```

Errors:

| `error` | When |
| --- | --- |
| `no capture port '<name>'` | `input` is not in the `capture` list of `ports`; nothing changes. |
| `no playback port '<name>'` | `output_left` or `output_right` is not in the `playback` list; nothing changes. |
| `cannot save <path>: <reason>` | `state.toml` cannot be written; nothing changes. |
| `cannot connect <from> -> <to>: <reason>` | A link failed after the names were stored; the other links are made and `ports` shows the result. Several failures are joined with `; `. |
| `cannot disconnect <from> -> <to>: <reason>` | Removing an old link failed after the names were stored; the other links are made and `ports` shows the result. Several failures are joined with `; `. |

## settings

Request:

```json
{"cmd":"settings"}
```

Response:

```json
{"ok":true,"bank_kind":"guitar","bank_dir":"/home/you/.config/xguitare/bank","ir_dir":"/home/you/.config/xguitare/bank","projects_dir":"/home/you/Music/xguitare"}
```

| Key | Value |
| --- | --- |
| `bank_kind` | `guitar` or `bass`. |
| `bank_dir` | The Bank directory in use. |
| `ir_dir` | The IR directory `ir_list` lists. |
| `projects_dir` | The folder that holds one sub-folder per Project. |

## set_settings

Stores and applies `bank_kind`, `ir_dir`, `projects_dir`, or any of them; a key left out keeps its value. All are stored in `state.toml`; `config.toml` is not changed. The Revision increments. A request that changes nothing (no key, the same `bank_kind`, the `ir_dir` already stored) writes nothing and leaves the Revision as it is.

- `bank_kind` different from the current one switches the Bank. The Bank directory in use is stored as the current kind's in `state.toml` `[banks]`; the new kind uses the directory `[banks]` holds for it, else `<config dir>/bank-<kind>` (`bank-guitar` or `bank-bass`), where `<config dir>` holds the config file, and that is stored too. A missing directory is first created with that kind's starter Presets and cabinet IR. Program 0 of the new Bank then plays; when it is missing, invalid or does not build, the passthrough Rack runs and no Preset is active. When `ir_dir` is set neither in `config.toml` nor through `set_settings`, the IR directory follows the Bank directory. The same `bank_kind` as now changes nothing and is not stored.
- `ir_dir` must be an absolute path to an existing directory; `ir_list` lists it from then on, and Cabinet IR names are looked up there first. The Rack playing now is not rebuilt.
- `projects_dir` must be an absolute path; it is created when missing. `project_new`, `project_open` and `project_list` use it from then on; an open Project stays where it is.

Request:

```json
{"cmd":"set_settings","bank_kind":"bass"}
```

Response: as `settings`, after the change.

```json
{"ok":true,"bank_kind":"bass","bank_dir":"/home/you/.config/xguitare/bank-bass","ir_dir":"/home/you/.config/xguitare/bank-bass"}
```

Errors (on each, the Daemon's state and `state.toml` are unchanged):

| `error` | When |
| --- | --- |
| `bank_dir is set in config.toml; edit it there` | `bank_kind` would switch the Bank, but the config file names `bank_dir`. |
| `ir_dir '<path>' is not a directory` | `ir_dir` is relative, does not exist or is not a directory. |
| `projects_dir must be an absolute path` | `projects_dir` is relative. |
| `cannot create projects_dir <path>: <reason>` | `projects_dir` is missing and cannot be created. |
| `cannot create bank <dir>: <reason>`, `cannot read bank <dir>: <reason>` | The new Bank directory cannot be seeded or read; the Bank is not switched, but a partly seeded directory may stay. |
| `cannot save <path>: <reason>` | `state.toml` cannot be written. |
| `engine busy` | The new Bank's Rack could not be queued; `state.toml` is put back as it was. |
| `invalid request: unknown variant ...` | `bank_kind` is neither `guitar` nor `bass`. |

## project_new

Creates a Project folder `<projects_dir>/<slug>/` with `project.toml` and an empty `takes/`, and opens it. The slug is the name lowercased, with ASCII letters and digits kept and every other run of characters one `-` (the rule `save` with `as` uses), so a name never reaches outside `projects_dir`. The Project's sample rate is the Engine's.

Request:

```json
{"cmd":"project_new","name":"Demo Song"}
```

Response:

```json
{"ok":true,"name":"Demo Song","path":"/home/you/Music/xguitare/demo-song"}
```

Errors:

| `error` | When |
| --- | --- |
| `a project is open; close it first` | A Project is open. |
| `a project is still closing; try again` | A Project that recorded and closed less than about 200 ms ago is still getting its last Takes added and saved. The reply carries `"code":"still_closing"`. |
| `project name must not be empty` | The name has no ASCII letter or digit. |
| `a project named '<name>' already exists` | The folder is already there. |
| `<path>: <reason>` | A folder or file cannot be written. |
| `<error>; the project was created, open it with project_open` | The folder was written but the Project could not start playing, e.g. `engine busy; the project was created, open it with project_open`. |

## project_open

Opens the Project in `<projects_dir>/<slug of name>/`: loads its active Takes into memory, connects each Track's input (an input that is not a capture port of another client stays unconnected, with a warning in the Daemon log), and plays it instead of the active Preset's Rack. The transport starts stopped at frame 0, with the click and count-in the Project saved. With `"restore":true` it loads `project.autosave.toml` instead of `project.toml`; the Project then counts as changed. An Item whose active Take holds no frames after its `offset`, or fewer than `offset + length`, plays the newest of its Takes that does, with `track '<name>': item <id> now plays <take file>: <old take file> ends before the item starts`; the Item keeps all its Takes. An Item none of whose Takes does is left out, with `track '<name>': dropped item <id> (<take file>, ...): starts past the end of its take`; its Take files stay on disk. Each such warning is in the reply's `warnings` and in the Daemon log.

Request:

```json
{"cmd":"project_open","name":"Demo Song"}
```

Response:

```json
{"ok":true,"autosave_newer":false,"name":"Demo Song","path":"/home/you/Music/xguitare/demo-song","restored":false,"warnings":[]}
```

| Key | Value |
| --- | --- |
| `autosave_newer` | `true` when `project.autosave.toml` is newer than `project.toml`: changes since the last save are in the autosave, and a client offers to open it again with `restore`. |
| `restored` | Whether the autosave was loaded. |
| `warnings` | What loading changed to open a file an earlier build wrote, one text each as above; empty when nothing. |

Errors:

| `error` | When |
| --- | --- |
| `a project is open; close it first` | A Project is open. |
| `a project is still closing; try again` | As `project_new`. |
| `no project '<name>'` | The folder has no `project.toml`, e.g. `no project '../etc'`. |
| `project rate <rate> Hz differs from the engine rate <rate> Hz` | The Project was recorded at another sample rate. |
| `cannot load take <file>: <reason>` | A Take file cannot be read or is not mono. |
| the `project.toml` error | The file cannot be read or a value is out of range, e.g. `<path>: line 1:8: ...`, `project format 2 is newer than this xguitare (reads up to 1)` (the file's top-level `format` is above 1; a file without it is format 1), `project format must be a whole number of 1 or more`, `tempo must be between 20 and 300 bpm`, `take file '../x.wav' must be takes/<name>.wav`, a Track name `track_add` would refuse (`a track named 'Bass' already exists`, `track name must not be only digits`; control characters and surrounding blanks are dropped), `<path>/project.autosave.toml: No such file or directory (os error 2)` with `restore` and no autosave. |
| `engine busy` | The audio thread's command queue is full; retry. |

## project_save

Writes the open Project to its `project.toml`, replacing it through a file renamed into place.

Request:

```json
{"cmd":"project_save"}
```

Response:

```json
{"ok":true,"path":"/home/you/Music/xguitare/demo-song/project.toml"}
```

Errors: `no project open`; `<path>: <reason>` when the file cannot be written.

## project_close

Saves the open Project to `project.toml`, stops playing it, cancels a running Render and goes back to the active Preset's Rack. The Daemon then adds the Takes still being written once they are finished and saves `project.toml` again; when the Project was recorded since it was opened, `project_new` and `project_open` answer `a project is still closing; try again` until then.

Request:

```json
{"cmd":"project_close"}
```

Response:

```json
{"ok":true,"name":"Demo Song","path":"/home/you/Music/xguitare/demo-song"}
```

Errors: `no project open`; `<path>: <reason>` when `project.toml` cannot be written (the Project stays open); `engine busy`.

## project_undo

Takes back the newest edit of the open Project's undo history and puts it on the redo history; `project_redo` does the newest undone edit again. Each answers with the edit's label and bumps the Revision; while playing, Item changes cross over in 5 ms as an edit does.

The history holds, per open Project, the last 200 of these edits, each one entry:

| Edit | Label |
| --- | --- |
| `item_move` | `move Item`, `move <n> Items` |
| `item_trim` | `trim` |
| `item_split` | `split` |
| `item_delete` | `delete Item`, `delete <n> Items` |
| `item_take` | `take <n>`, the Take counted from 1 |
| `item_duplicate` | `duplicate Item`, `duplicate <n> Items` |
| `item_paste` | `paste Item`, `paste <n> Items`, counting the Items placed |
| A Take added after recording | `record <Track>` |
| `track_add` | `add Track <name>` |
| `track_remove` | `remove Track <name>`; undone, the Track comes back at its place with its Rack, mixer values, Sends and Items, selected if it was |

Mixer, Rack, tempo, loop, click, Bus and Track rename requests do not enter the history and leave it as it is. A new edit clears the redo history. The history is not saved and is cleared when the Project closes. A Track that comes back (an undone `track_remove` or a redone `track_add`) drops its Sends to Buses removed since, and when another Track has taken its name since, it comes back as `<name> 2` (or the next free number); the reply and the label then name it so. A recording enters the history once its Takes are added, about a second after `stop` (when they show up in `items`). Undoing a recorded Take removes its Item and keeps its file in `takes/`; Takes stay in memory until the Project closes, and one that is not is loaded before an undo or redo that plays it. When that fails (`cannot load take <file>: <reason>`, e.g. after the file was deleted outside the Daemon), the entry stays where it is and undo (or redo) answers the same until the file is back, so older entries cannot be undone meanwhile. An Item keeps its id through undo and redo, and a new Item never gets the id of one that undo took away.

Request:

```json
{"cmd":"project_undo"}
```

Response:

```json
{"ok":true,"label":"move 2 Items"}
```

Errors: `no project open`; `cannot undo while recording` (`cannot redo while recording`) after `record`, until `stop` or `play`; `nothing to undo` (`nothing to redo`); `engine busy` (nothing changed, both histories stay; retry); `cannot load take <file>: <reason>`.

## project_redo

Does the newest undone edit again; see `project_undo`.

Request:

```json
{"cmd":"project_redo"}
```

Response:

```json
{"ok":true,"label":"move 2 Items"}
```

## project_list

Lists the folders under `projects_dir` that hold a `project.toml`, sorted by folder name.

Request:

```json
{"cmd":"project_list"}
```

Response:

```json
{"ok":true,"projects":[{"folder":"demo-song","name":"Demo Song","valid":true}]}
```

| Key | Value |
| --- | --- |
| `name` | The Project's name; the folder name when its file is invalid. |
| `folder` | The folder name; `project_open` takes it or the name. |
| `valid`, `error` | Whether `project.toml` loads, and why not. |

Errors: `<path>: <reason>` when `projects_dir` exists but cannot be read.

## items

Lists the open Project's Items, Track by Track. A client fetches it again when the `status` Revision changes: a new Take, a loop Take and `project_close` all change it.

Request:

```json
{"cmd":"items"}
```

Response:

```json
{"ok":true,"sample_rate":48000,"tracks":[{"id":0,"name":"Guitar","items":[{"id":3,"position":96000,"bars_beats":"2.1","offset":0,"length":192000,"take":1,"takes":["takes/guitar-1.wav","takes/guitar-2.wav"],"take_frames":192000}]},{"id":1,"name":"Bass","items":[]}]}
```

| Key | Value |
| --- | --- |
| `sample_rate` | The Project's sample rate, to turn frames into seconds. |
| `tracks` | One entry per Track in order: `id`, which stays the Track's while other Tracks are added or removed, `name` and its `items`. |
| `id` | The Item's id, unique in the Project and never reused; it stays the Item's while other Items are added or removed. |
| `position` | Frames from the Project start where the Item begins; `bars_beats` is the same as `<bar>.<beat>`, as in `status`. |
| `offset` | Frames of the active Take skipped before `position`. |
| `length` | Frames the Item plays: its own length when it has one, else the active Take's frames less `offset`. |
| `take` | Index in `takes` of the active Take, the one that plays. |
| `takes` | The Take files, relative to the Project folder, oldest first. |
| `take_frames` | Frames of the active Take. |

Errors: `no project open`.

## Item edits

`item_move`, `item_trim`, `item_split`, `item_delete`, `item_take`, `item_duplicate` and `item_paste` change the open Project's Items, named by their `id` from `items`. Each one bumps the `status` Revision and marks the Project changed, also when nothing moved. While the transport plays, the Tracks whose Items changed cross over to the new Items in 5 ms; the Racks are not rebuilt and the playhead keeps going. Times are frames from the Project start; a client turns `<bar>.<beat>` or seconds into frames with the Project's `sample_rate` and `tempo`.

All but `item_delete` answer with the Items they changed or made, each as `items` lists it plus `track`, the zero-based index of its Track, so a client can follow a moved Item, both halves of a split or the new copies:

```json
{"ok":true,"items":[{"track":0,"id":3,"position":96000,"bars_beats":"2.1","offset":0,"length":48000,"take":1,"takes":["takes/guitar-1.wav","takes/guitar-2.wav"],"take_frames":192000},{"track":0,"id":7,"position":144000,"bars_beats":"2.3","offset":48000,"length":144000,"take":1,"takes":["takes/guitar-1.wav","takes/guitar-2.wav"],"take_frames":192000}]}
```

Errors every Item edit can return:

| `error` | When |
| --- | --- |
| `no project open` | No Project is open. |
| `cannot edit items while recording` | After `record`, until `stop` or `play`. |
| `no such item: <id>` | No Item of the open Project has that id. |
| `give at least one item` | `ids` is empty. |
| `engine busy` | The audio thread's command queue has no slot for every Track that changes; nothing changed, retry. |
| `cannot load take <file>: <reason>` | The Take that would play is not in memory and its file cannot be read. |
| `track '<name>' item <n> ends past the last frame` | The edit would put an Item past the last frame a Project holds. |

## item_move

Moves the Items `ids` by `delta` frames (negative is earlier). All move by the same amount: when one would start before frame 0, the amount is cut so that the earliest starts at frame 0. The moved Items go on top of their Track (the end of its Items, in the order they were stacked), so they are heard over what they land on. With `track` (a zero-based index or a name), the first Item in `ids` moves to that Track and the others keep their distance in Tracks to it. `offset`, `length` and the Takes stay as they are.

Request:

```json
{"cmd":"item_move","ids":[3,4],"delta":-48000,"track":"Bass"}
```

Errors, besides those above: `no track '<track>'`; `item <id> would move to a track that does not exist` when keeping the Track distance would put that Item before the first or after the last Track.

## item_trim

Moves an Item's start (`start`) or end (`end`), or both, to these timeline frames; a missing one stays. Each is clamped to what the active Take holds: the start to no earlier than where the Take's first frame lies (`position` − `offset`, and frame 0), the end to no later than where its last frame lies. Moving the start changes `position` and `offset` together, so the audio stays where it was on the timeline. The Item keeps its place in the stack.

Request:

```json
{"cmd":"item_trim","id":3,"start":100000,"end":180000}
```

Errors, besides those above: `item <id> would have no frames left` when the clamped end is not after the clamped start.

## item_split

Splits each Item that the frame `at` falls strictly inside (after its first frame, before its end) into two at `at`. Without `ids`, the Items under `at` on the selected Track are split. Both halves keep all the Item's Takes and its active Take; the left one keeps the id, the right one gets a new id and sits just above the left one in the stack. An Item of `ids` that `at` is not inside is left as it is. The reply lists each left half followed by its right half.

Request:

```json
{"cmd":"item_split","ids":[3],"at":144000}
```

Errors, besides those above: `nothing to split at <frame>` when `at` is inside none of the Items.

## item_delete

Removes the Items `ids`. Their Take files stay in `takes/`.

Request:

```json
{"cmd":"item_delete","ids":[3,7]}
```

Response:

```json
{"ok":true,"deleted":[3,7]}
```

## item_take

Makes the Take at index `take` (zero-based, as `items` `take`) the Item's active one, loading it when it is not in memory. An Item with a `length` that the new Take is too short for is cut to the frames that Take holds after the Item's `offset`.

Request:

```json
{"cmd":"item_take","id":3,"take":0}
```

Errors, besides those above: `item <id> has no take <take>`; `item <id> starts past the end of take <take>` when that Take holds no frames after the Item's `offset`.

## item_duplicate

Copies the Items `ids` so that the copies start where the group ends: the group runs from the earliest start to the latest end of those Items, and each copy lands that many frames later, on its own Item's Track, on top. The copies keep the Takes, the active Take, `offset` and `length`, and get new ids. The reply lists the copies in Track order, and in stacking order within a Track.

Request:

```json
{"cmd":"item_duplicate","ids":[3,4]}
```

## item_copy

Puts the Items `ids` in the open Project's clipboard, replacing what it held: each Item with its distance in frames to the group's start (the earliest start of those Items) and its distance in Tracks to the Track of the first Item in `ids`. It is not an edit: the Project, the Revision and the undo history stay as they are, and it works while recording. The clipboard keeps the Items as they were copied, also when they are edited or deleted afterwards, until the next `item_copy` or until the Project closes. `status` `clipboard` counts its Items.

Request:

```json
{"cmd":"item_copy","ids":[3,4]}
```

Response:

```json
{"ok":true,"copied":[3,4]}
```

Errors: `no project open`; `no such item: <id>`; `give at least one item`.

## item_paste

Places the clipboard's Items as new Items, with new ids, on top of their Tracks: the first Item copied on `track` (a zero-based index or a name), the others at their distance in Tracks to it, and the group's start at the frame `at`, the others keeping their distance to it. An Item whose Track would be before the first or after the last Track is left out; `dropped` counts them. Paste as often as you like; the clipboard stays.

Request:

```json
{"cmd":"item_paste","track":"Bass","at":192000}
```

Response:

```json
{"ok":true,"items":[{"track":1,"id":9,"position":192000,"bars_beats":"3.1","offset":0,"length":48000,"take":0,"takes":["takes/bass-1.wav"],"take_frames":48000}],"dropped":1}
```

Errors, besides those above: `clipboard is empty` before any `item_copy` in the open Project; `no track '<track>'`; `cannot paste: take <file> no longer exists` when a Take file of an Item to place is gone from the Project folder (nothing is placed).

## take_peaks

Pages of the peak table of a Take of the open Project, for drawing its waveform: the lowest and highest sample of every 256 frames, the last partial run included, scaled to −127…127. The Daemon builds the table when the Take is loaded or finished, and again when a new recording reuses a Take's file name; a client keeps a table for the `frames` it was fetched for and fetches it again when `items` gives that file other `take_frames`.

Request:

```json
{"cmd":"take_peaks","file":"takes/guitar-2.wav","first":0,"count":16384}
```

`file` is a Take file as `items` lists it. `first` is the first pair (default 0); `count` the pairs wanted (default and most 16384).

Response:

```json
{"ok":true,"frames_per_peak":256,"frames":191872,"total":750,"first":0,"peaks":[-3,4,-80,96,-61,70]}
```

| Key | Value |
| --- | --- |
| `frames_per_peak` | Frames one pair covers: 256. |
| `frames` | The Take's frames the table was built from. |
| `total` | Pairs in the whole table. |
| `first` | The first pair of this page, as asked. |
| `peaks` | `[min, max, …]`, one pair per 256 frames from `first`; empty when `first` is at or past `total`. |

Errors: `no project open`; `no such take: <file>` for a file that is not a loaded Take of the open Project.

## takes_unused

Lists the open Project's Take files that nothing uses any more: regular files `takes/<name>.wav` directly in its `takes/` folder that no Item names in any of its Takes (active or not), that the clipboard does not hold, and that are not being recorded or still being written. Symlinks, subfolders, other files and names with a control character or a bidi override or isolate (U+202A–U+202E, U+2066–U+2069) are never listed. Sorted by name. A Project without a `takes/` folder lists nothing.

Request:

```json
{"cmd":"takes_unused"}
```

Response:

```json
{"ok":true,"files":[{"file":"takes/guitar-2.wav","bytes":1234567},{"file":"takes/guitar-3.wav","bytes":800}],"bytes":1235367}
```

| Key | Value |
| --- | --- |
| `files` | Each unused Take file as `takes/<name>.wav`, with its size in `bytes`. |
| `bytes` | The total size. |

Errors: `no project open`; `takes is not a directory` when `takes` is a symlink or a file; `cannot read <path>: <reason>`.

## takes_clean

Deletes Take files that `takes_unused` lists. Every name is checked first, against the unused list as it is now: one refused name refuses the whole request, and an error reply means no Take file was deleted (when removing the autosave fails, `project.toml` has already been written). Then the Project is saved (`project.toml` written, the autosave removed), so no saved file names a deleted Take; the named Takes leave memory, the undo and redo histories and the clipboard are cleared, and the Revision goes up. Then each file is checked again and removed: a symlink in `takes/` is never followed, and a file that is no longer a regular file or cannot be removed is listed in `failed` while the others are still deleted. Take files are deleted only by this request. A request line is limited to 64 KiB, so clients send a large clean-up in batches; the CLI and the GUI keep each batch's file names under 32 KiB and stop at the first batch that is refused. Each batch is its own request: when a later batch is refused, the files of the earlier batches stay deleted.

Request:

```json
{"cmd":"takes_clean","files":["takes/guitar-2.wav","takes/guitar-3.wav"]}
```

`files` are Take files as `takes_unused` lists them; a name given twice counts once. An empty list deletes nothing and does not save.

Response:

```json
{"ok":true,"deleted":1,"bytes":800,"failed":[{"file":"takes/guitar-2.wav","error":"Permission denied (os error 13)"}]}
```

| Key | Value |
| --- | --- |
| `deleted` | The number of files removed. |
| `bytes` | Their total size. |
| `failed` | Each file that was not removed, with the reason: `not a regular file`, or the system's error; empty when every file went. |

Errors: `no project open`; `cannot clean takes while recording` after `record`, until `stop` or `play`; `invalid take file '<file>'` for a name that is not `takes/<name>.wav` or holds `..`, a control character, or a bidi override or isolate; `not an unused take: <file>` for a file that is not listed as unused (also one replaced by a symlink after listing); `takes is not a directory`; the errors of `project_save`; `cannot remove <autosave>: <reason>`; `cannot read <path>: <reason>`. In `invalid take file`, control characters and bidi overrides and isolates are escaped as `\u{202e}`.

## track_add

Adds a Track at the end with the active Preset's Modules (none when no Preset is active), no input, 0 dB, centred, and selects it. Without `name` it is `Track <n>`, the first number free.

Request:

```json
{"cmd":"track_add","name":"Bass"}
```

Response:

```json
{"ok":true,"index":1,"name":"Bass"}
```

Errors:

| `error` | When |
| --- | --- |
| `no project open` | No Project is open. |
| `a track named '<name>' already exists` | Another Track has the name, in any letter case. |
| `track name must not be empty` | The name is empty after control characters and surrounding blanks are removed. |
| `track name must not be only digits` | The name is all digits, which the CLI reads as an index. |
| `a project has at most 16 tracks` | The Project has 16 Tracks. |
| `engine busy` | The audio thread's command queue is full; retry. |

## track_remove

Removes a Track (`track`: index or name, see Track scope). Its Take files stay in `takes/`. A Take of another Track that the disk is still writing lands on that Track wherever it moved.

Request:

```json
{"cmd":"track_remove","track":"Bass"}
```

Response:

```json
{"ok":true,"index":1,"name":"Bass"}
```

Errors: `no project open`; `no track '<track>'`; `cannot remove a track while recording` after `record` until `stop` or `play`; `engine busy`.

## track_set

Changes a Track; each key left out keeps its value. Every value is checked before anything changes.

| Key | Value |
| --- | --- |
| `track` | Index or name (required). |
| `input` | A capture port name as `ports` lists it; `""` disconnects the Track's input. |
| `volume` | dB, −60 to 6. |
| `pan` | −1 (left) to 1 (right), constant power. |
| `mute`, `solo` | Booleans; while any Track is soloed, only soloed Tracks sound. |
| `arm` | Boolean: an armed Track monitors its input through its Rack and records while recording. |
| `name` | A new name, unique in any letter case. |
| `to_master` | Boolean: whether the Track's own sound goes to Master; its Sends go to their Buses either way. |

Request:

```json
{"cmd":"track_set","track":0,"input":"Scarlett Solo 4th Gen Input 1 Inst/Line:capture_MONO","volume":-3,"pan":-0.25,"arm":true}
```

Response: the Track after the change.

```json
{"ok":true,"arm":true,"index":0,"input":"Scarlett Solo 4th Gen Input 1 Inst/Line:capture_MONO","mute":false,"name":"Track 1","pan":-0.25,"solo":false,"to_master":true,"volume_db":-3.0}
```

Errors:

| `error` | When |
| --- | --- |
| `no project open`, `no track '<track>'` | See Track scope. |
| `track '<name>' volume must be between -60 and 6 dB` | `volume` is out of range. |
| `track '<name>' pan must be between -1 and 1` | `pan` is out of range. |
| `no capture port '<input>'` | `input` is not a capture port. |
| `cannot change an armed track's input while recording` | `input` is given for a Track that is or becomes armed, after `record` until `stop` or `play`. |
| `a track named '<name>' already exists`, `track name must not be empty`, `track name must not be only digits` | As `track_add`. |
| `engine busy` | The audio thread's command queue is full; retry. |

## track_select

Selects the Track that Rack requests without `track` act on.

Request:

```json
{"cmd":"track_select","track":"Bass"}
```

Response:

```json
{"ok":true,"index":1,"name":"Bass"}
```

Errors: `no project open`; `no track '<track>'`.

## transport

Runs the transport. `action` is one of:

- `play`: plays from the playhead; armed Tracks keep monitoring.
- `stop`: stops; the playhead stays where it is.
- `record`: records every armed Track; from stopped, after `count_in` bars of click (see `click`). Each Take becomes an Item placed earlier by the input and output latency and the Project's `latency_offset_ms`. A Take that holds no frames after the ones this skips (recorded from frame 0 and stopped within the latency) becomes no Item; its file is kept and listed by `takes_unused`.
- `locate`: moves the playhead to `position`: a frame (JSON number), or text `<bar>.<beat>`, `<bar>` (beat 1), or `<seconds>s`, with bars and beats counted from 1 at the Project's tempo and meter. It may be at most one hour past the end of the Project's last Take or loop region.

Request:

```json
{"cmd":"transport","action":"locate","position":"2.1"}
```

Response: `action`, and for `locate` the frame (here at 100 bpm in 3/4).

```json
{"ok":true,"action":"locate","frames":86400}
```

Errors:

| `error` | When |
| --- | --- |
| `no project open` | No Project is open. |
| `no track is armed` | `record` with no armed Track. |
| `project rate <rate> Hz differs from the engine rate <rate> Hz` | `play` or `record` while the Engine runs at another sample rate than the Project. |
| `locate needs a position` | `locate` without `position`. |
| `position must be <bar>.<beat> or <seconds>s` | The text is neither, the bar or beat is 0, the beat is past the meter, or the seconds are negative or not finite. |
| `position is more than an hour past the end of the project` | The position is past that limit, e.g. `1e300s` or frame 18446744073709551615. |
| `engine busy` | The audio thread's command queue is full; retry. |

## loop

Sets the loop region in frames. While it is on and the playhead is before `end`, the playhead wraps from `end` to `start`; while recording, each pass becomes another Take of the same Item.

Request:

```json
{"cmd":"loop","start":0,"end":172800,"on":true}
```

Response:

```json
{"ok":true,"end":172800,"on":true,"start":0}
```

Errors: `no project open`; `loop start must be before loop end` when on; `loop region ends past the last frame`; `engine busy`.

## tempo

Sets the tempo in beats per minute and the meter `[<beats per bar>, <note value>]`; a key left out keeps its value. Both are saved with the Project.

Request:

```json
{"cmd":"tempo","bpm":100,"meter":[3,4]}
```

Response:

```json
{"ok":true,"bpm":100.0,"meter":[3,4]}
```

Errors: `no project open`; `tempo must be between 20 and 300 bpm`; `meter must be 1 to 16 beats of a 2, 4, 8 or 16 note`; `engine busy`.

## click

Turns the click (a short blip on every beat, higher and 6 dB louder on beat 1, at the Metronome volume; see `metronome`) on or off and sets the count-in: 0, 1 or 2 bars of click before `record` starts from stopped. A key left out keeps its value. Both are saved with the Project (`click`, `count_in` in `project.toml`).

Request:

```json
{"cmd":"click","on":true,"count_in":1}
```

Response:

```json
{"ok":true,"count_in":1,"on":true}
```

Errors: `no project open`; `count-in must be 0, 1 or 2 bars`; `engine busy`.

## metronome

Changes the Metronome, a short blip on every beat, beat 1 of each bar higher (1.5 kHz instead of 1 kHz) and 6 dB louder, added to the outputs. A key left out keeps its value. With no Project open it plays over the Rack at its own `bpm` (20 to 300, decimals allowed) and `meter` (as a Project's); turning it `on` starts at beat 1 of a bar, and a new tempo or meter keeps the beat count. `volume` is dB, −40 to 0 (default −12), and applies to the Project click too. Mute silences it; it keeps counting, so it comes back in time. `bpm`, `meter` and `volume` are saved in `state.toml` (`[metronome]`: `bpm`, `meter`, `volume_db`); the Metronome always starts off.

While a Project is open the Metronome is the Project's click: `on` is the `click` request's `on`, and `bpm` and `meter` are the `tempo` request's, saved with the Project. Opening a Project (`project_new`, `project_open`) switches the Metronome off, and it stays off after `project_close`; its own tempo and meter are kept and come back then.

Request:

```json
{"cmd":"metronome","on":true,"bpm":90,"meter":[3,4],"volume":-12}
```

Response, the new state as in `status`:

```json
{"ok":true,"beat":0,"bpm":90.0,"meter":[3,4],"on":true,"volume_db":-12.0}
```

Errors: `bpm must be between 20 and 300`; `meter must be 1 to 16 beats of a 2, 4, 8 or 16 note`; `volume must be between -40 and 0 dB`; `engine busy`; `cannot save <state.toml path>: <reason>`.

## metronome_tap

Taps the tempo. The tempo is the average interval of the last taps, up to four, rounded to 0.1 bpm and clamped to 20–300; a gap over 2 seconds starts a new series. From the second tap of a series it sets the tempo as `metronome` with `bpm` does (the Project's tempo while one is open); after the first `bpm` is `null` and nothing changes.

Request:

```json
{"cmd":"metronome_tap"}
```

Response:

```json
{"ok":true,"bpm":120.0}
```

Errors: as `metronome`.

## bus_add

Adds a Bus at the end of the open Project: an empty stereo Rack, 0 dB, centred, mute and solo off, and an off Send to it from every Track. Without `name` it is `Bus <n>`, the first number free from 1. Bus names follow the Track name rules.

Request:

```json
{"cmd":"bus_add","name":"Verb"}
```

Response:

```json
{"ok":true,"bus":{"id":0,"index":0,"name":"Verb"}}
```

| Key | Value |
| --- | --- |
| `bus` | The new Bus: its `id` (see `status` `buses`), zero-based `index` and `name`. |

Errors:

| `error` | When |
| --- | --- |
| `no project open` | No Project is open. |
| `a bus named '<name>' already exists` | Another Bus has the name, in any letter case. |
| `bus name must not be empty` | The name is empty after control characters and surrounding blanks are removed. |
| `bus name must not be only digits` | The name is all digits, which the CLI reads as an index. |
| `at most 8 buses` | The Project has 8 Buses. |
| `engine busy` | The audio thread's command queue is full; retry. |

## bus_remove

Removes a Bus (`bus`: index or name) and every Send to it. The Buses after it move up one index and keep their ids.

Request:

```json
{"cmd":"bus_remove","bus":"Verb"}
```

Response: the Bus removed, as `bus_add` answers.

```json
{"ok":true,"bus":{"id":0,"index":0,"name":"Verb"}}
```

Errors: `no project open`; `no bus '<bus>'`; `engine busy`.

## bus_set

Changes a Bus; each key left out keeps its value. Every value is checked before anything changes.

| Key | Value |
| --- | --- |
| `bus` | Index or name (required). |
| `name` | A new name, unique among the Buses in any letter case. |
| `volume` | dB, −60 to 6. |
| `pan` | −1 (left) to 1 (right), constant power. |
| `mute`, `solo` | Booleans; solo is solo in place (see `send_set`). |

Request:

```json
{"cmd":"bus_set","bus":0,"volume":-3,"solo":true}
```

Response: the Bus after the change.

```json
{"ok":true,"bus":{"id":0,"index":0,"mute":false,"name":"Verb","pan":0.0,"solo":true,"volume_db":-3.0}}
```

Errors:

| `error` | When |
| --- | --- |
| `no project open`, `no bus '<bus>'` | See Track scope. |
| `bus '<name>' volume must be between -60 and 6 dB` | `volume` is out of range. |
| `bus '<name>' pan must be between -1 and 1` | `pan` is out of range. |
| `a bus named '<name>' already exists`, `bus name must not be empty`, `bus name must not be only digits` | As `bus_add`. |
| `engine busy` | The audio thread's command queue is full; retry. |

## send_set

Changes a Track's Send to a Bus; each key left out keeps its value. Every Track has one Send per Bus, off until a `level` is given.

| Key | Value |
| --- | --- |
| `track` | Index or name of the Track (required). |
| `bus` | Index or name of the Bus (required). |
| `level` | dB, −60 to 6, or `"off"`. |
| `pre` | Boolean: `false` (the default) takes the Send after the Track's volume and pan, `true` after its Rack and pan, before its volume. |

Solo is solo in place: while any Track or Bus is soloed, the soloed Tracks and Buses sound, with the Buses a soloed Track sends to and the Tracks that send to a soloed Bus (through a Send that is not off). Mute always wins, on the Track's own sound and on its Sends.

Request:

```json
{"cmd":"send_set","track":"Guitar","bus":"Verb","level":-6}
```

Response:

```json
{"ok":true,"bus":0,"level":-6.0,"pre":false,"track":0}
```

| Key | Value |
| --- | --- |
| `track`, `bus` | The Track's and the Bus's index. |
| `level` | The Send's level in dB, or `"off"`. |
| `pre` | Whether it is taken before the Track's volume. An off Send is not saved, so its `pre` is lost with the Project's next save. |

Errors:

| `error` | When |
| --- | --- |
| `no project open`, `no track '<track>'`, `no bus '<bus>'` | See Track scope. |
| `track '<name>' send level must be -inf or between -60 and 6 dB` | `level` is a number out of range. |
| `invalid request: level must be a number in dB or "off"` | `level` is a string other than `"off"` (followed by the parser's position). |
| `engine busy` | The audio thread's command queue is full; retry. |

## master_set

Sets the Master volume: `volume` is dB, −60 to 6, or `"off"`. Master is a stereo Rack (empty in a new Project) with this volume; its output goes to the outputs, and the click is added after it.

Request:

```json
{"cmd":"master_set","volume":-3}
```

Response:

```json
{"ok":true,"volume_db":-3.0}
```

`volume_db` is `null` when off.

Errors:

| `error` | When |
| --- | --- |
| `no project open` | No Project is open. |
| `master volume must be -inf or between -60 and 6 dB` | `volume` is a number out of range. |
| `invalid request: level must be a number in dB or "off"` | `volume` is a string other than `"off"` (followed by the parser's position). |
| `engine busy` | The audio thread's command queue is full; retry. |

## clip_reset

Clears the clip flag (`status` `clip`) of a Track (`track`), a Bus (`bus`) or Master (`"master":true`), or of all when none is given. The Revision does not change.

Request:

```json
{"cmd":"clip_reset","bus":"Verb"}
```

Response: the index cleared, as `{"track": <index>}`, `{"bus": <index>}` or `{"master": true}`, or nothing more when all were cleared.

```json
{"ok":true,"bus":0}
```

Removing a Track or Bus clears the clip flags of the Tracks or Buses after it, whose indexes move.

Errors: `no project open` (a `track`, `bus` or `master` given while no Project is open); `no track '<track>'`; `no bus '<bus>'`; `give track, bus or master, not more than one`.

## render

Starts a Render of the open Project in the background: its current state (unsaved changes included) plays at the Project's sample rate through its Tracks, Buses and Master, faster than real time, into a stereo WAV, while the Daemon goes on playing and recording. The click and the live inputs are never rendered; an armed Track renders its Items. One Render runs at a time.

- `range`: `"project"` (default) from frame 0 to the end of the last Item, then a tail until Master stays below −90 dBFS for 0.5 s, 10 s at most; `"loop"` from the loop start to its end exactly, whether the loop is on or not.
- `bits`: `24` (default), `16` (with triangular dither) or `"32f"` (32-bit float).
- `name`: the file name without `.wav`; by default the Project folder's name. It must not be empty, start with `.`, or contain `/` or `..`.

The file is `<project folder>/renders/<name>.wav`; `renders/` is created when missing. An existing file is never replaced: `<name>-2.wav`, `<name>-3.wav`, … is used instead, also when the file appears while the Render runs. The Render writes `<file>.part`, created new, and links it to the file when done; on a filesystem without hard links (exFAT, vfat) it creates the file new and renames the part file onto it. A cancelled or failed Render removes the part file.

Request:

```json
{"cmd":"render","range":"loop","bits":16,"name":"Chorus"}
```

Response: `file`, the WAV it is writing. Follow `status` `render` for its progress and end; the Daemon logs `rendered <file>` or `render failed: <reason>`.

```json
{"ok":true,"file":"/home/you/Music/xguitare/demo-song/renders/Chorus.wav"}
```

Errors:

| `error` | When |
| --- | --- |
| `no project open` | No Project is open. |
| `a render is already running` | Another Render has not ended yet. |
| `nothing to render` | No Track has an Item. |
| `no loop region` | `range` is `loop` and the loop start is not before its end. |
| `invalid render name '<name>'` | `name` is empty, starts with `.`, or contains `/`, `..` or a control character (shown escaped, as `\n`). |
| `renders is not a directory` | The Project folder's `renders` is a symlink or not a directory. |
| `invalid request: bits must be 16, 24 or "32f" …` | `bits` has another value. |

## render_cancel

Stops the running Render; it removes its part file and `status` `render` `state` turns `cancelled`. Closing the Project or stopping the Daemon does the same.

Request:

```json
{"cmd":"render_cancel"}
```

Response:

```json
{"ok":true}
```

Errors: `no render running`.
