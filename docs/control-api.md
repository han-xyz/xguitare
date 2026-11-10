# Control API

The protocol a client uses to inspect and change the running Daemon: which Preset is active, the Bank, Parameter values and the Rack's layout. The `xguitare` CLI is one client of it; a GUI or script can be another. Request and response types are in `crates/xguitare-core/src/control.rs`; the socket server is `crates/xguitare/src/socket_server.rs`.

## Transport

- A Unix stream socket at the config key `socket`, by default `$XDG_RUNTIME_DIR/xguitare/xguitare.sock` (`/tmp/xguitare-<uid>/xguitare.sock` when `$XDG_RUNTIME_DIR` is unset).
- Each request is one JSON object on one line, ended by `\n`. Each request gets exactly one response: one JSON object on one line, ended by `\n`. Requests on one connection are answered in order.
- The connection stays open after a response; send the next request on it, or close it. The CLI opens one connection per command.
- A line that is not a valid request (malformed JSON, an unknown `cmd`, a missing or wrongly typed field, an empty line) gets an error response starting with `invalid request: ` followed by the JSON parser's message, and the connection stays open.
- A line longer than 64 KiB gets `{"ok":false,"error":"request too long"}` and the Daemon closes the connection.
- The Daemon serves at most 32 connections at once. One more gets `{"ok":false,"error":"too many connections"}`, without sending a request, and is closed.
- A connection that sends nothing for 300 seconds is closed by the Daemon; a client that keeps one open reconnects.
- One Daemon per socket: the Daemon holds an exclusive `flock` on `<socket>.lock` (e.g. `xguitare.sock.lock`) while it runs, and a second Daemon on the same socket exits with `error: another Daemon is running`.

Every response has a boolean `ok`. On success it carries the request's payload keys next to `"ok":true`; on failure it is exactly `{"ok":false,"error":"<message>"}`. Key order in responses is not significant.

Errors any request can return:

| `error` | When |
| --- | --- |
| `invalid request: <parser message>` | The line is not a valid request, e.g. `invalid request: expected ident at line 1 column 2` for `not json`, ``invalid request: unknown variant `reboot`, expected one of `status`, `list`, `load`, `set`, `get`, `save`, `modules`, `rack_add`, `rack_remove`, `rack_move`, `tuner`, `mute`, `bypass`, `ir_list`, `rack_ir`, `ports`, `set_ports`, `settings`, `set_settings`, `project_new`, `project_open`, `project_save`, `project_close`, `project_list`, `items`, `track_add`, `track_remove`, `track_set`, `track_select`, `transport`, `loop`, `tempo`, `click` at line 1 column 15``, ``invalid request: missing field `preset` ``. |
| `request too long` | The line exceeds 64 KiB; the connection is then closed. |
| `too many connections` | Sent unasked on connect when 32 connections are already open; the connection is then closed. |

Parameter addresses are `<module id>.<param name>`, e.g. `preamp.gain`. A Module's id is its `id` in the Preset file, or its type when the file gives none.

### Track scope

While a Project is open the Engine plays its Tracks, not the active Preset's Rack. `load`, `set`, `get`, `bypass`, `rack_add`, `rack_remove`, `rack_move` and `rack_ir` then act on one Track's Modules: the one named by an optional `track` key, else the selected Track (see `track_select`). `track` is a zero-based index (a JSON number) or a Track name (a JSON string). `load` copies the Preset's Modules into the Track. The active Preset, `modified` and `save` stay as they were; the Track's changes are saved with the Project.

```json
{"cmd":"set","param":"preamp.gain","value":20,"track":"Bass"}
```

Errors of the Track scope, on any of these requests:

| `error` | When |
| --- | --- |
| `no project open` | `track` is given while no Project is open. |
| `the project has no tracks` | A Project is open, `track` is left out and the Project has no Track. |
| `no track '<track>'` | No Track has that index or name, e.g. `no track '5'`. |

## status

Request:

```json
{"cmd":"status"}
```

Response:

```json
{"ok":true,"buffer_size":128,"callback_last_us":926,"callback_max_us":1484,"click":{"count_in":1,"on":true},"connections":[{"from":"Scarlett Solo 4th Gen Input 1 Inst/Line:capture_MONO","to":"xguitare:in"},{"from":"xguitare:out_l","to":"Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FL"},{"from":"xguitare:out_r","to":"Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FR"}],"count_in_remaining":0,"disk_overflows":0,"input_peak_db":-96.1,"loaded_audio_bytes":3840000,"loop":{"end":172800,"on":true,"start":0},"modified":false,"muted":false,"output_peak_db":-120.0,"position":{"bars_beats":"1.1","frames":0},"prepared_frames":128,"preset":"Clean","program":0,"project":"Demo Song","project_warning":null,"revision":10,"sample_rate":48000,"selected":1,"tempo":{"bpm":100.0,"meter":[3,4]},"tracks":[{"armed":true,"input":"Scarlett Solo 4th Gen Input 1 Inst/Line:capture_MONO","input_peak_db":-120.0,"mute":false,"name":"Track 1","pan":-0.25,"solo":false,"volume_db":-3.0},{"armed":false,"input":null,"input_peak_db":-120.0,"mute":false,"name":"Bass","pan":0.0,"solo":false,"volume_db":0.0}],"transport":"stopped","xruns":0}
```

| Key | Value |
| --- | --- |
| `preset` | Name of the active Preset; `null` when none is loaded (empty Bank, or the first Preset failed to load at start). |
| `program` | Bank position of the active Preset's file; `null` when no Preset is loaded or its file has been removed from the Bank. The `preset` name stays while the removed Preset keeps playing. |
| `sample_rate` | JACK sample rate in Hz; follows a rate change while the Daemon runs, and the Rack and the Tuner are rebuilt at the new rate. An open Project stops playing while the rate differs from its own (see `project_warning`) and plays again once they match. |
| `buffer_size` | JACK buffer size in frames, as currently granted. |
| `prepared_frames` | Block size in frames the running Rack was prepared for. The Daemon re-prepares the Rack, and an open Project's Tracks, within a second when `buffer_size` differs. |
| `revision` | The Revision: starts at 1 and increments on every successful `load`, `set`, `bypass`, `save`, `rack_add`, `rack_remove`, `rack_move` and `rack_ir`, on a hot reload of the active Preset, on a Bank re-index and on a re-prepare; with Projects also on `project_new`, `project_open`, `project_close`, `track_add`, `track_remove`, `track_set`, `track_select`, `loop`, `tempo`, `click`, a Rack request on a Track, and each Take added to the open Project. A client refreshes when it changes. `mute` does not change it. |
| `modified` | Whether the live Rack has been changed since the active Preset was last loaded, reloaded or saved: `true` after a successful `set`, `bypass`, `rack_add`, `rack_remove`, `rack_move` or `rack_ir`; `false` after a `load`, a `save` (with or without `as`) or a hot reload of the active Preset, and when no Preset is loaded. |
| `muted` | Whether Mute is on (see `mute`). |
| `input_peak_db` | Largest input sample since the previous `status` on the same connection (for the first `status` on a connection, since the previous `status` on any connection), in dB (`20·log10`), one decimal; `-120.0` for silence. Each `status` resets it for its own connection only. |
| `output_peak_db` | As `input_peak_db`, over both outputs. |
| `xruns` | Xruns since the Daemon started. |
| `callback_max_us` | Longest audio callback so far, in µs. |
| `callback_last_us` | Most recent audio callback, in µs. |
| `connections` | Port connections the Daemon made at start, each `{"from": <port>, "to": <port>}`. |
| `disk_overflows` | Times since the Daemon started that a Track's recording ring was full and frames were dropped because the disk fell behind. |
| `count_in_remaining` | Beats left in the count-in before recording starts, rounded up; `0` when no count-in is under way or no Project is open. |
| `loaded_audio_bytes` | Bytes of Take audio the open Project holds in memory (4 per frame); `0` when no Project is open. |
| `project_warning` | Why the open Project does not play, e.g. `project rate 48000 Hz differs from the engine rate 44100 Hz` after a sample-rate change: the transport stops, Takes still open are finished, and `play` and `record` are refused until the rates match; `null` otherwise. |
| `project` | Name of the open Project; `null` when none is open. The keys below are `null` (`tracks` empty) while no Project is open. |
| `transport` | `stopped`, `playing` or `recording` (also during the count-in), as the audio thread runs it. |
| `position` | The playhead: `frames` from the Project start, counted from 0, and `bars_beats` as `<bar>.<beat>`, counted from 1 at the Project's tempo and meter. |
| `tempo` | `bpm` and `meter` `[<beats per bar>, <note value>]`. |
| `loop` | The loop region: `start` and `end` in frames, and whether it is `on`. |
| `click` | Whether the click is `on`, and `count_in` bars before recording. |
| `selected` | Index of the selected Track. |
| `tracks` | One entry per Track in order: `name`, `input` (capture port or `null`), `volume_db`, `pan`, `mute`, `solo`, `armed`, and `input_peak_db` of its input port, measured as `input_peak_db`. |

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
{"ok":true,"params":[{"bypass":false,"default":40.0,"integer":false,"max":100.0,"min":0.0,"module":"preamp","param":"gain","type":"preamp","unit":"%","value":100.0}]}
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
| `params[].integer` | `true` when the Parameter takes whole numbers only (`preamp.stages`, `delay.pingpong`, `cabinet.low_cut`, `cabinet.high_cut`); `set` and the Module round other values. |
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
{"ok":true,"modules":[{"channels":"mono","params":[{"default":0.0,"integer":false,"max":20.0,"min":-60.0,"name":"level","unit":"dB"}],"type":"gain"},{"channels":"mono_to_stereo","params":[{"default":0.0,"integer":false,"max":20.0,"min":-20.0,"name":"level","unit":"dB"},{"default":20.0,"integer":true,"max":300.0,"min":20.0,"name":"low_cut","unit":"Hz"},{"default":20000.0,"integer":true,"max":20000.0,"min":2000.0,"name":"high_cut","unit":"Hz"}],"type":"cabinet"}]}
```

| Key | Value |
| --- | --- |
| `modules[].type` | The Module type, as written in a Preset's `type`. |
| `modules[].channels` | `"mono"` (before the cabinet), `"stereo"` (after it) or `"mono_to_stereo"` (the cabinet). |
| `modules[].params` | The Parameters in order, each with `name`, `min`, `max`, `default`, `unit` (as in `get`) and `integer`. |

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
| `a project is still closing; try again` | A Project that recorded and closed less than about 200 ms ago is still getting its last Takes added and saved. |
| `project name must not be empty` | The name has no ASCII letter or digit. |
| `a project named '<name>' already exists` | The folder is already there. |
| `<path>: <reason>` | A folder or file cannot be written. |
| `<error>; the project was created, open it with project_open` | The folder was written but the Project could not start playing, e.g. `engine busy; the project was created, open it with project_open`. |

## project_open

Opens the Project in `<projects_dir>/<slug of name>/`: loads its active Takes into memory, connects each Track's input (an input that is not a capture port of another client stays unconnected, with a warning in the Daemon log), and plays it instead of the active Preset's Rack. The transport starts stopped at frame 0, with the click and count-in the Project saved. With `"restore":true` it loads `project.autosave.toml` instead of `project.toml`; the Project then counts as changed.

Request:

```json
{"cmd":"project_open","name":"Demo Song"}
```

Response:

```json
{"ok":true,"autosave_newer":false,"name":"Demo Song","path":"/home/you/Music/xguitare/demo-song","restored":false}
```

| Key | Value |
| --- | --- |
| `autosave_newer` | `true` when `project.autosave.toml` is newer than `project.toml`: changes since the last save are in the autosave, and a client offers to open it again with `restore`. |
| `restored` | Whether the autosave was loaded. |

Errors:

| `error` | When |
| --- | --- |
| `a project is open; close it first` | A Project is open. |
| `a project is still closing; try again` | As `project_new`. |
| `no project '<name>'` | The folder has no `project.toml`, e.g. `no project '../etc'`. |
| `project rate <rate> Hz differs from the engine rate <rate> Hz` | The Project was recorded at another sample rate. |
| `cannot load take <file>: <reason>` | A Take file cannot be read or is not mono. |
| the `project.toml` error | The file cannot be read or a value is out of range, e.g. `<path>: line 1:8: ...`, `tempo must be between 20 and 300 bpm`, `take file '../x.wav' must be takes/<name>.wav`, a Track name `track_add` would refuse (`a track named 'Bass' already exists`, `track name must not be only digits`; control characters and surrounding blanks are dropped), `<path>/project.autosave.toml: No such file or directory (os error 2)` with `restore` and no autosave. |
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

Saves the open Project to `project.toml`, stops playing it and goes back to the active Preset's Rack. The Daemon then adds the Takes still being written once they are finished and saves `project.toml` again; when the Project was recorded since it was opened, `project_new` and `project_open` answer `a project is still closing; try again` until then.

Request:

```json
{"cmd":"project_close"}
```

Response:

```json
{"ok":true,"name":"Demo Song","path":"/home/you/Music/xguitare/demo-song"}
```

Errors: `no project open`; `<path>: <reason>` when `project.toml` cannot be written (the Project stays open); `engine busy`.

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
{"ok":true,"sample_rate":48000,"tracks":[{"id":0,"name":"Guitar","items":[{"position":96000,"bars_beats":"2.1","offset":0,"length":192000,"take":1,"takes":["takes/guitar-1.wav","takes/guitar-2.wav"]}]},{"id":1,"name":"Bass","items":[]}]}
```

| Key | Value |
| --- | --- |
| `sample_rate` | The Project's sample rate, to turn frames into seconds. |
| `tracks` | One entry per Track in order: `id`, which stays the Track's while other Tracks are added or removed, `name` and its `items`. |
| `position` | Frames from the Project start where the Item begins; `bars_beats` is the same as `<bar>.<beat>`, as in `status`. |
| `offset` | Frames of the active Take skipped before `position`. |
| `length` | Frames the Item plays: the active Take's frames less `offset`. |
| `take` | Index in `takes` of the active Take, the one that plays. |
| `takes` | The Take files, relative to the Project folder, oldest first. |

Errors: `no project open`.

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

Request:

```json
{"cmd":"track_set","track":0,"input":"Scarlett Solo 4th Gen Input 1 Inst/Line:capture_MONO","volume":-3,"pan":-0.25,"arm":true}
```

Response: the Track after the change.

```json
{"ok":true,"arm":true,"index":0,"input":"Scarlett Solo 4th Gen Input 1 Inst/Line:capture_MONO","mute":false,"name":"Track 1","pan":-0.25,"solo":false,"volume_db":-3.0}
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
- `record`: records every armed Track; from stopped, after `count_in` bars of click (see `click`). Each Take becomes an Item placed earlier by the input and output latency and the Project's `latency_offset_ms`.
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

Turns the click (a short blip on every beat, higher on beat 1) on or off and sets the count-in: 0, 1 or 2 bars of click before `record` starts from stopped. A key left out keeps its value. Both are saved with the Project (`click`, `count_in` in `project.toml`).

Request:

```json
{"cmd":"click","on":true,"count_in":1}
```

Response:

```json
{"ok":true,"count_in":1,"on":true}
```

Errors: `no project open`; `count-in must be 0, 1 or 2 bars`; `engine busy`.
