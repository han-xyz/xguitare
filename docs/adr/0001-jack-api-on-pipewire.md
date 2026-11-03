# Audio I/O through the JACK API, running on pipewire-jack

The target machine runs PipeWire 1.6, and the audio stack is tuned for it (allowed rates, zero ALSA headroom on the Scarlett). We talk to it through the JACK client API rather than native libpipewire: the process-callback model is small and stable, routing tools (qpwgraph, carla) and real JACK servers work unchanged, and the engine only needs a fixed-size float buffer callback plus port registration. We give up fine control over quantum negotiation and PipeWire-only features (per-node latency hints beyond what the JACK shim exposes, graph metadata); if those become necessary the audio adapter is the one module to swap.

## Considered Options

- Native libpipewire filter: most control, largest and least stable API, PipeWire-only.
- cpal / RtAudio: portable, but weak low-latency and routing control on Linux.
