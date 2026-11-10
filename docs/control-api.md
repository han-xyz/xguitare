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
| `invalid request: <parser message>` | The line is not a valid request, e.g. `invalid request: expected ident at line 1 column 2` for `not json`, ``invalid request: unknown variant `reboot`, expected one of `status`, `list`, `load`, `set`, `get`, `save`, `modules`, `rack_add`, `rack_remove`, `rack_move`, `tuner`, `mute`, `ir_list`, `rack_ir`, `ports`, `set_ports`, `settings`, `set_settings` at line 1 column 15``, ``invalid request: missing field `preset` ``. |
| `request too long` | The line exceeds 64 KiB; the connection is then closed. |
| `too many connections` | Sent unasked on connect when 32 connections are already open; the connection is then closed. |

Parameter addresses are `<module id>.<param name>`, e.g. `preamp.gain`. A Module's id is its `id` in the Preset file, or its type when the file gives none.

## status

Request:

```json
{"cmd":"status"}
```

Response:

```json
{"ok":true,"buffer_size":128,"callback_last_us":741,"callback_max_us":1228,"connections":[{"from":"Scarlett Solo 4th Gen Input 1 Inst/Line:capture_MONO","to":"xguitare:in"},{"from":"xguitare:out_l","to":"Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FL"},{"from":"xguitare:out_r","to":"Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FR"}],"input_peak_db":-103.8,"modified":false,"muted":false,"output_peak_db":-120.0,"prepared_frames":128,"preset":"Clean","program":0,"revision":2,"sample_rate":48000,"xruns":0}
```

| Key | Value |
| --- | --- |
| `preset` | Name of the active Preset; `null` when none is loaded (empty Bank, or the first Preset failed to load at start). |
| `program` | Bank position of the active Preset's file; `null` when no Preset is loaded or its file has been removed from the Bank. The `preset` name stays while the removed Preset keeps playing. |
| `sample_rate` | JACK sample rate in Hz; follows a rate change while the Daemon runs, and the Rack and the Tuner are rebuilt at the new rate. |
| `buffer_size` | JACK buffer size in frames, as currently granted. |
| `prepared_frames` | Block size in frames the running Rack was prepared for. The Daemon re-prepares the Rack within a second when `buffer_size` differs. |
| `revision` | The Revision: starts at 1 and increments on every successful `load`, `set`, `save`, `rack_add`, `rack_remove`, `rack_move` and `rack_ir`, on a hot reload of the active Preset, on a Bank re-index and on a re-prepare. A client refreshes when it changes. `mute` does not change it. |
| `modified` | Whether the live Rack has been changed since the active Preset was last loaded, reloaded or saved: `true` after a successful `set`, `rack_add`, `rack_remove`, `rack_move` or `rack_ir`; `false` after a `load`, a `save` (with or without `as`) or a hot reload of the active Preset, and when no Preset is loaded. |
| `muted` | Whether Mute is on (see `mute`). |
| `input_peak_db` | Largest input sample since the previous `status` on the same connection (for the first `status` on a connection, since the previous `status` on any connection), in dB (`20·log10`), one decimal; `-120.0` for silence. Each `status` resets it for its own connection only. |
| `output_peak_db` | As `input_peak_db`, over both outputs. |
| `xruns` | Xruns since the Daemon started. |
| `callback_max_us` | Longest audio callback so far, in µs. |
| `callback_last_us` | Most recent audio callback, in µs. |
| `connections` | Port connections the Daemon made at start, each `{"from": <port>, "to": <port>}`. |

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
{"ok":true,"params":[{"default":40.0,"integer":false,"max":100.0,"min":0.0,"module":"preamp","param":"gain","type":"preamp","unit":"%","value":100.0}]}
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
{"ok":true,"bank_kind":"guitar","bank_dir":"/home/you/.config/xguitare/bank","ir_dir":"/home/you/.config/xguitare/bank"}
```

| Key | Value |
| --- | --- |
| `bank_kind` | `guitar` or `bass`. |
| `bank_dir` | The Bank directory in use. |
| `ir_dir` | The IR directory `ir_list` lists. |

## set_settings

Stores and applies `bank_kind`, `ir_dir`, or both; a key left out keeps its value. Both are stored in `state.toml`; `config.toml` is not changed. The Revision increments. A request that changes nothing (no key, the same `bank_kind`, the `ir_dir` already stored) writes nothing and leaves the Revision as it is.

- `bank_kind` different from the current one switches the Bank. The Bank directory in use is stored as the current kind's in `state.toml` `[banks]`; the new kind uses the directory `[banks]` holds for it, else `<config dir>/bank-<kind>` (`bank-guitar` or `bank-bass`), where `<config dir>` holds the config file, and that is stored too. A missing directory is first created with that kind's starter Presets and cabinet IR. Program 0 of the new Bank then plays; when it is missing, invalid or does not build, the passthrough Rack runs and no Preset is active. When `ir_dir` is set neither in `config.toml` nor through `set_settings`, the IR directory follows the Bank directory. The same `bank_kind` as now changes nothing and is not stored.
- `ir_dir` must be an absolute path to an existing directory; `ir_list` lists it from then on, and Cabinet IR names are looked up there first. The Rack playing now is not rebuilt.

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
| `cannot create bank <dir>: <reason>`, `cannot read bank <dir>: <reason>` | The new Bank directory cannot be seeded or read; the Bank is not switched, but a partly seeded directory may stay. |
| `cannot save <path>: <reason>` | `state.toml` cannot be written. |
| `engine busy` | The new Bank's Rack could not be queued; `state.toml` is put back as it was. |
| `invalid request: unknown variant ...` | `bank_kind` is neither `guitar` nor `bass`. |
