import { describe, test, expect, beforeAll } from 'vitest';
import { sonioxClient, sonioxMicClient } from '../src/js/soniox.js';
import { TranscriptUI } from '../src/js/ui.js';

// Minimal DOM stubs so TranscriptUI runs under vitest's node environment.
beforeAll(() => {
    globalThis.document = {
        createElement: () => {
            const el = { innerHTML: '', appendChild() {}, remove() {} };
            Object.defineProperty(el, 'textContent', {
                set(v) { this.innerHTML = String(v); },
            });
            return el;
        },
    };
    Object.defineProperty(globalThis, 'navigator', {
        value: { platform: 'Win32' },
        configurable: true,
    });
});

function makeContainer() {
    return {
        innerHTML: '',
        scrollHeight: 0,
        scrollTop: 0,
        clientHeight: 0,
        parentElement: null,
        style: { setProperty() {} },
        querySelector: () => null,
        querySelectorAll: () => [],
        appendChild() {},
    };
}

describe('dual Soniox clients', () => {
    test('mic client is an independent SonioxClient instance', () => {
        expect(sonioxMicClient).not.toBe(sonioxClient);
        sonioxClient.isConnected = true;
        expect(sonioxMicClient.isConnected).toBe(false);
        sonioxClient.isConnected = false;
    });
});

describe('per-source transcript state', () => {
    test('provisional state is tracked per source', () => {
        const ui = new TranscriptUI(makeContainer());
        ui.addOriginal('hello', 'S1', 'en', 'system');
        ui.setProvisional('draft', 'S2', 'vi', 'mic');
        expect(ui.provisionalBySource.mic.text).toBe('draft');
        expect(ui.provisionalText).toBe(''); // system tail untouched
    });

    test('translation aligns to oldest untranslated of the same source', () => {
        const ui = new TranscriptUI(makeContainer());
        ui.addOriginal('hello', 'S1', 'en', 'system');
        ui.addOriginal('xin chao', null, 'vi', 'mic');
        ui.addTranslation('mic-translated', 'mic');
        const micSeg = ui.sessionLog.find(s => s.source === 'mic' && s.original === 'xin chao');
        expect(micSeg.translation).toBe('mic-translated');
        const sysSeg = ui.sessionLog.find(s => s.source === 'system');
        expect(sysSeg.translation).toBeNull();
    });

    test('committedTextBySource filters sessionLog by source', () => {
        const ui = new TranscriptUI(makeContainer());
        ui.addOriginal('q one', 'S1', 'en', 'system');
        ui.addOriginal('my answer', null, 'en', 'mic');
        expect(ui.committedTextBySource('system')).toBe('q one');
        expect(ui.committedTextBySource('mic')).toBe('my answer');
    });

    test('legacy segments without source still receive translations', () => {
        const ui = new TranscriptUI(makeContainer());
        ui.loadSegments([{ original: 'old', translation: null, status: 'original', createdAt: 1 }]);
        ui.addTranslation('old-translated', 'system');
        expect(ui.sessionLog[0].translation).toBe('old-translated');
    });
});

