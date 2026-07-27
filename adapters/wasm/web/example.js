// Sketch after `wasm-pack build adapters/wasm --release --target web`.
// Import paths and worklet wiring depend on your application bundler.
import init, { WasmStreamingEngine, memory } from "../pkg/glowbeat_wasm.js";

await init();

const sampleRate = 48000;
const channels = 2;
const lightCount = 128;
const maxCallbackScalars = 4096;
const engine = new WasmStreamingEngine(
  sampleRate,
  channels,
  lightCount,
  maxCallbackScalars,
);

let memoryBuffer = null;
let inputView;
let outputView;
function refreshViews() {
  if (memoryBuffer !== memory.buffer) {
    memoryBuffer = memory.buffer;
    inputView = new Float32Array(
      memoryBuffer,
      engine.input_ptr(),
      engine.input_capacity(),
    );
    outputView = new Float32Array(
      memoryBuffer,
      engine.output_ptr(),
      engine.output_len(),
    );
  }
}

export function processInterleavedPcm(interleaved, absoluteSamplePosition) {
  refreshViews();
  if (interleaved.length > inputView.length) {
    throw new RangeError("Audio callback block exceeds configured input capacity");
  }
  inputView.set(interleaved, 0);
  engine.push_input(interleaved.length);
  engine.render(absoluteSamplePosition);
  refreshViews();
  return outputView; // flattened r,g,b,a; consume before the next render
}
