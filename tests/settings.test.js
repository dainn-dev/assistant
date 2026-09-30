import { describe, it, expect, vi, beforeEach } from 'vitest';
import { settingsManager } from '../src/js/settings.js';

const { invoke } = window.__TAURI__.core;

describe('settingsManager', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    settingsManager.settings = { ...settingsManager.settings };
  });

  it('load() merges backend settings over defaults', async () => {
    invoke.mockResolvedValueOnce({ target_language: 'ja', font_size: 20 });
    const s = await settingsManager.load();
    expect(s.target_language).toBe('ja'); // backend wins
    expect(s.font_size).toBe(20);
    expect(s.source_language).toBe('auto'); // default preserved
  });

  it('load() falls back to defaults when the backend fails', async () => {
    invoke.mockRejectedValueOnce(new Error('no backend'));
    const s = await settingsManager.load();
    expect(s.source_language).toBe('auto');
    expect(s.target_language).toBe('vi');
  });

  it('save() sends merged settings to save_settings', async () => {
    invoke.mockResolvedValueOnce(null);
    await settingsManager.save({ font_size: 22 });
    expect(invoke).toHaveBeenCalledWith(
      'save_settings',
      expect.objectContaining({
        newSettings: expect.objectContaining({ font_size: 22 }),
      })
    );
    expect(settingsManager.get().font_size).toBe(22);
  });

  it('save() propagates backend errors', async () => {
    invoke.mockRejectedValueOnce('Unknown settings field(s): bogus');
    await expect(settingsManager.save({ bogus: 1 })).rejects.toMatch(/Unknown settings field/);
  });
});
