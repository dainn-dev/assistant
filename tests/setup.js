// Vitest global setup — stub the Tauri bridge that frontend modules
// destructure at import time (const { invoke } = window.__TAURI__.core).
import { vi } from 'vitest';

globalThis.window = globalThis.window || {};
window.__TAURI__ = {
  core: { invoke: vi.fn(async () => null) },
  event: { listen: vi.fn(async () => () => {}) },
  window: { getCurrentWindow: vi.fn() },
  dialog: { open: vi.fn(async () => null) },
};
