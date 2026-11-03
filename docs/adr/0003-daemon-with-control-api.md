# Engine lives in a Daemon reached over a Unix-socket Control API

Control (CLI now, GUI and DAW later) runs out of process and talks to the Daemon over a Unix socket carrying JSON lines. This keeps every UI off the audio process, lets the headless engine be tested and scripted before any GUI exists, and lets the GUI crash or restart without a dropout. The cost is a serialisation boundary and a protocol to version; an in-process GUI would be simpler to build first but would couple UI framework choice to the real-time core, which we want to pick later.
