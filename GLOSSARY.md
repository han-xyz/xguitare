# xguitare

A live guitar amp and effects rack. One guitar in, stereo out, as little delay as the hardware allows.

## Language

**Rack**:
The ordered list of Modules a signal passes through, from guitar input to stereo output.
_Avoid_: chain, pipeline, graph, signal path

**Module**:
One sound-shaping unit in a Rack with a fixed set of Parameters: gate, drive, preamp, tonestack, cabinet, delay, reverb.
_Avoid_: effect, plugin, node, block, stomp

**Parameter**:
A named control of a Module with a defined range and unit. `param` is the accepted short form in code and in the Control API (`<module>.<param>`, `params`).
_Avoid_: knob, setting, control

**Preset**:
A named, saved Rack: its Module order and every Parameter value.
_Avoid_: patch, scene, tone, program

**Bank**:
An ordered collection of Presets; a Preset's position in it is its MIDI program number.
_Avoid_: set list, library, collection

**Cabinet IR**:
An impulse response audio file that the cabinet Module convolves the signal with.
_Avoid_: cab sim, speaker model, IR file

**Engine**:
The real-time part that runs one Rack on the audio thread.
_Avoid_: core, backend, processor, DSP thread

**Daemon**:
The long-running process that owns the Engine, the audio client and the Control API.
_Avoid_: server, service, host

**Control API**:
The protocol a client (CLI, later GUI) uses to inspect and change the Daemon's Preset and Parameters.
_Avoid_: RPC, IPC, socket API, remote

**Revision**:
A counter the Daemon increments on every change to the live Rack or the Bank, so a client knows when to refresh.
_Avoid_: version, generation, sequence number

**Program**:
A Preset's position in the Bank, the number a MIDI program change or a digit key selects.
_Avoid_: slot, index
