# Control API

The protocol a client uses to inspect and change the running Daemon: which Preset is active, the Bank, and Parameter values. The `xguitare` CLI is one client of it; a GUI or script can be another. Request and response types are in `crates/xguitare-core/src/control.rs`; the socket server is `crates/xguitare/src/socket_server.rs`.

## Transport

- A Unix stream socket at the config key `socket`, by default `$XDG_RUNTIME_DIR/xguitare/xguitare.sock` (`/tmp/xguitare-<uid>/xguitare.sock` when `$XDG_RUNTIME_DIR` is unset).
- Each request is one JSON object on one line, ended by `\n`. Each request gets exactly one response: one JSON object on one line, ended by `\n`. Requests on one connection are answered in order.
- The connection stays open after a response; send the next request on it, or close it. The CLI opens one connection per command.
- A line that is not a valid request (malformed JSON, an unknown `cmd`, a missing or wrongly typed field, an empty line) gets an error response starting with `invalid request: ` followed by the JSON parser's message, and the connection stays open.
- A line longer than 64 KiB gets `{"ok":false,"error":"request too long"}` and the Daemon closes the connection.

Every response has a boolean `ok`. On success it carries the request's payload keys next to `"ok":true`; on failure it is exactly `{"ok":false,"error":"<message>"}`. Key order in responses is not significant.

Errors any request can return:

| `error` | When |
| --- | --- |
| `invalid request: <parser message>` | The line is not a valid request, e.g. `invalid request: expected ident at line 1 column 2` for `not json`, ``invalid request: unknown variant `reboot`, expected one of `status`, `list`, `load`, `set`, `get`, `save` at line 1 column 15``, ``invalid request: missing field `preset` ``. |
| `request too long` | The line exceeds 64 KiB; the connection is then closed. |

Parameter addresses are `<module id>.<param name>`, e.g. `preamp.gain`. A Module's id is its `id` in the Preset file, or its type when the file gives none.

## status

Request:

```json
{"cmd":"status"}
```

Response:

```json
{"ok":true,"buffer_size":128,"callback_last_us":741,"callback_max_us":1228,"connections":[{"from":"Scarlett Solo 4th Gen Input 1 Inst/Line:capture_MONO","to":"xguitare:in"},{"from":"xguitare:out_l","to":"Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FL"},{"from":"xguitare:out_r","to":"Scarlett Solo 4th Gen Headphones / Line 1-2:playback_FR"}],"input_peak_db":-103.8,"output_peak_db":-120.0,"prepared_frames":128,"preset":"Clean","program":0,"revision":2,"sample_rate":48000,"xruns":0}
```

| Key | Value |
| --- | --- |
| `preset` | Name of the active Preset; `null` when none is loaded (empty Bank, or the first Preset failed to load at start). |
| `program` | Bank position of the active Preset's file; `null` when no Preset is loaded or its file has been removed from the Bank. The `preset` name stays while the removed Preset keeps playing. |
| `sample_rate` | JACK sample rate in Hz. |
| `buffer_size` | JACK buffer size in frames, as currently granted. |
| `prepared_frames` | Block size in frames the running Rack was prepared for. The Daemon re-prepares the Rack within a second when `buffer_size` differs. |
| `revision` | The Revision: starts at 1 and increments on every successful `load`, `set` and `save`, on a hot reload of the active Preset, on a Bank re-index and on a re-prepare. A client refreshes when it changes. |
| `input_peak_db` | Largest input sample since the previous `status`, in dB (`20·log10`), one decimal; `-120.0` for silence. Each `status` resets it. |
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

Switches the running Rack to a Bank entry, with a crossfade. `preset` is a program number (all digits) or a Preset name (exact match, first in Bank order).

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

Changes one Parameter of the running Rack. The value is clamped to the Parameter's range. The change is not written to the Preset file until `save`.

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
| `value` | The value applied, after clamping. |
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
{"ok":true,"params":[{"integer":false,"max":100.0,"min":0.0,"module":"preamp","param":"gain","unit":"%","value":100.0}]}
```

| Key | Value |
| --- | --- |
| `params` | The selected Parameters in Rack order, then Parameter order. |
| `params[].module` | Module id. |
| `params[].param` | Parameter name. |
| `params[].value` | Current value. |
| `params[].unit` | `"dB"`, `"ms"`, `"Hz"`, `"%"`, or `""` for a plain number. |
| `params[].min`, `params[].max` | The Parameter's range. |
| `params[].integer` | `true` when the Parameter takes whole numbers only (`preamp.stages`, `delay.pingpong`); the Module rounds other values. |

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

Errors:

| `error` | When |
| --- | --- |
| `no preset loaded` | No Preset is active. |
| `active preset file was removed; load a preset first` | The active Preset's file is no longer in the Bank. |
| `cannot save <path>: <reason>` | Writing or renaming the file failed. |
