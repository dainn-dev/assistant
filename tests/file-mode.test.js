import { describe, test, expect, vi, beforeEach } from 'vitest';
import { fileModeMethods, fileProgressLabel } from '../src/js/file-mode.js';

function fakeApp() {
    const app = { toasts: [], ...fileModeMethods };
    app._showToast = (message, type = 'success') => app.toasts.push({ message, type });
    return app;
}

beforeEach(() => {
    window.__TAURI__.core.Channel = class { constructor() { this.onmessage = null; } };
});

describe('fileProgressLabel', () => {
    test('names each stage and counts translation batches', () => {
        expect(fileProgressLabel({ stage: 'decoding' })).toBe('Reading audio…');
        expect(fileProgressLabel({ stage: 'transcribing' })).toBe('Transcribing with Deepgram…');
        expect(fileProgressLabel({ stage: 'translating', current: 20, total: 57 }))
            .toBe('Translating subtitles… 20/57');
        expect(fileProgressLabel({ stage: 'writing' })).toBe('Saving subtitles…');
        expect(fileProgressLabel({ stage: 'unknown' })).toBeNull();
    });
});

describe('_transcribeFile', () => {
    test('reports where both subtitle files were written', async () => {
        window.__TAURI__.core.invoke = vi.fn(async () => ({
            originalSrt: 'C:\\v\\talk.ja.srt', translatedSrt: 'C:\\v\\talk.vi.srt', cues: 3, audioSeconds: 65,
        }));
        const app = fakeApp();

        await app._transcribeFile('C:\\v\\talk.mp4');

        expect(window.__TAURI__.core.invoke).toHaveBeenCalledWith(
            'transcribe_media_file', expect.objectContaining({ path: 'C:\\v\\talk.mp4' }));
        const last = app.toasts.at(-1);
        expect(last.type).toBe('success');
        expect(last.message).toContain('talk.vi.srt');
        expect(last.message).toContain('3 lines');
    });

    test('surfaces backend errors as error toasts and frees the job slot', async () => {
        window.__TAURI__.core.invoke = vi.fn(async () => { throw 'No speech was found in the file.'; });
        const app = fakeApp();

        await app._transcribeFile('a.mp3');

        expect(app.toasts.at(-1)).toEqual({ message: 'No speech was found in the file.', type: 'error' });
        expect(app._fileJobRunning).toBe(false);
    });

    test('refuses a second file while one is still processing', async () => {
        let finish;
        window.__TAURI__.core.invoke = vi.fn(() => new Promise((resolve) => { finish = resolve; }));
        const app = fakeApp();

        const first = app._transcribeFile('a.mp3');
        await app._transcribeFile('b.mp3');

        expect(window.__TAURI__.core.invoke).toHaveBeenCalledTimes(1);
        expect(app.toasts.at(-1).type).toBe('error');
        finish({ originalSrt: 'a', translatedSrt: 'b', cues: 1, audioSeconds: 1 });
        await first;
    });
});
