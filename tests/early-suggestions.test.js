import { describe, test, expect, beforeAll, vi } from 'vitest';
import { sonioxClient, sonioxMicClient } from '../src/js/soniox.js';
import { TranscriptUI } from '../src/js/ui.js';
import { EarlyScheduler, normalizeSnapshot } from '../src/js/early-suggestions.js';

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

describe('EarlyScheduler', () => {
    const Q = 'tell me about how you handled deadlock in the project';
    const makeSched = (t0 = 0) => {
        let t = t0;
        const fired = [];
        const cancelled = [];
        const s = new EarlyScheduler({
            now: () => t,
            fire: (snap, meta) => { fired.push({ snap, meta }); return fired.length; },
            cancel: (id) => cancelled.push(id),
        });
        return { s, fired, cancelled, tick: (ms) => { t += ms; } };
    };

    test('needs 2 identical observations and a meaningful prefix', () => {
        const { s, fired } = makeSched();
        s.observe(Q, 'system');
        expect(fired).toHaveLength(0);          // single observation — not stable
        s.observe(Q, 'system');
        expect(fired).toHaveLength(1);
    });

    test('short fragments never fire', () => {
        const { s, fired } = makeSched();
        s.observe('so', 'system');
        s.observe('so', 'system');
        s.observe('so um', 'system');
        s.observe('so um', 'system');
        expect(fired).toHaveLength(0);
    });

    test('shrinking snapshot counts as correction and cancels in-flight', () => {
        const { s, fired, cancelled } = makeSched();
        s.observe('tell me about deadlock handling today', 'system');
        s.observe('tell me about deadlock handling today', 'system');
        expect(fired).toHaveLength(1);          // in-flight
        s.observe('tell me about', 'system');   // correction — shorter prefix
        expect(cancelled).toEqual([1]);
        expect(s.revision).toBe(1);
    });

    test('single-flight: new snapshot queues as pending, not a second request', () => {
        const { s, fired, tick } = makeSched();
        s.observe(Q, 'system');
        s.observe(Q, 'system');
        expect(fired).toHaveLength(1);
        const longer = Q + ' and the retry logic';
        s.observe(longer, 'system');
        s.observe(longer, 'system');            // stable, but in-flight → pending
        expect(fired).toHaveLength(1);
        tick(1600);                              // spacing window elapses
        s.requestDone(1);                        // complete → pending fires
        expect(fired).toHaveLength(2);
        expect(fired[1].snap).toBe(normalizeSnapshot(longer));
    });

    test('caps: max 3 fires per turn', () => {
        const { s, fired, tick } = makeSched();
        let snap = 'q';
        for (let i = 0; i < 6; i++) {
            tick(2000);
            snap += ` part${i}`;
            s.observe(snap, 'system');
            s.observe(snap, 'system');
            s.requestDone(s.inFlightId);
        }
        expect(fired.length).toBeLessThanOrEqual(3);
    });

    test('mic source never fires', () => {
        const { s, fired } = makeSched();
        s.observe(Q, 'mic');
        s.observe(Q, 'mic');
        expect(fired).toHaveLength(0);
    });

    test('endpoint allows one final fire on changed snapshot', () => {
        const { s, fired, tick } = makeSched();
        s.observe('what is', 'system');
        s.observe('what is', 'system');          // too short — no fire
        tick(100);
        s.observe('what is your approach to caching', 'system');
        s.endpoint('system');                    // question ended → fire once
        expect(fired).toHaveLength(1);
    });

    test('spacing: back-to-back stable snapshots respect 1.5s gap', () => {
        const { s, fired, tick } = makeSched();
        s.observe(Q, 'system');
        s.observe(Q, 'system');
        s.requestDone(1);
        const longer = Q + ' please';
        s.observe(longer, 'system');
        s.observe(longer, 'system');
        expect(fired).toHaveLength(1);           // <1.5s since first fire — held
        tick(1600);
        s.observe(longer, 'system');
        expect(fired).toHaveLength(2);
    });
});

