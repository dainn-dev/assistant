import { describe, test, expect, beforeAll, beforeEach, vi } from 'vitest';
import { sessionMethods } from '../src/js/session.js';
import { conversationMethods } from '../src/js/conversations.js';
import { toastMethods } from '../src/js/toast.js';
import { TranscriptUI } from '../src/js/ui.js';
import { settingsManager } from '../src/js/settings.js';
import { sonioxClient, sonioxMicClient } from '../src/js/soniox.js';

// ipc.js resolves window.__TAURI__ lazily at call time — stub it here and the
// seam picks it up without import-order gymnastics.
const invokeCalls = [];
let invokeImpl = async () => null;
globalThis.window = {
    __TAURI__: {
        core: {
            invoke: (cmd, args) => {
                invokeCalls.push({ cmd, args });
                return invokeImpl(cmd, args);
            },
            Channel: class {
                constructor() { this.onmessage = null; }
            },
        },
        event: { listen: async () => () => {} },
        window: { getCurrentWindow: () => ({}) },
    },
};

function stubEl() {
    return {
        style: {},
        classList: { add() {}, remove() {}, toggle() {}, contains: () => false },
        textContent: '',
        innerHTML: '',
        value: '',
        checked: false,
        disabled: false,
        hidden: false,
        addEventListener() {},
        removeEventListener() {},
        appendChild() {},
        remove() {},
        querySelector: () => null,
        querySelectorAll: () => [],
        setAttribute() {},
    };
}

beforeAll(() => {
    globalThis.document = {
        getElementById: () => stubEl(),
        querySelector: () => null,
        querySelectorAll: () => [],
        createElement: () => stubEl(),
        body: { classList: { add() {}, remove() {}, toggle() {} } },
        documentElement: { style: { setProperty() {}, removeProperty() {} } },
    };
    Object.defineProperty(globalThis, 'navigator', {
        value: { platform: 'Win32' },
        configurable: true,
    });
    globalThis.confirm = () => true;
    globalThis.localStorage = { getItem: () => null, setItem() {}, removeItem() {} };
});

function makeContainer() {
    return {
        innerHTML: '',
        scrollHeight: 0, scrollTop: 0, clientHeight: 0,
        parentElement: null,
        style: { setProperty() {} },
        querySelector: () => null,
        querySelectorAll: () => [],
        appendChild() {},
    };
}

function makeApp() {
    const toasts = [];
    const app = Object.assign({}, sessionMethods, conversationMethods, {
        isRunning: false,
        isStarting: false,
        sessionActive: false,
        activeConversationFilename: null,
        currentSource: 'system',
        currentTemplate: null,
        translationMode: 'soniox',
        ttsEnabled: false,
        _permissions: null,
        _early: null,
        _earlyMetrics: null,
        _earlyMicMarked: false,
        _micDegraded: false,
        _sessionFilename: null,
        _savedSessionJson: null,
        _lastSavedAt: null,
        _chipTimer: null,
        _sonioxSec: { system: 0, mic: 0 },
        _llmCalls: 0,
        recordingStartTime: null,
        sessionStartTime: null,
        transcriptUI: new TranscriptUI(makeContainer()),
        _toasts: toasts,
        _showToast: (m, t) => toasts.push({ m, t }),
        _updateStartButton() {},
        _updateControlsForMode() {},
        _updateSessionChip() {},
        _updateStatus() {},
        _showView() {},
        _isSuggestionsMode() { return this.currentTemplate === 'Interview'; },
        _startNewSessionFlow: sessionMethods._startNewSessionFlow,
    });
    return app;
}

beforeEach(() => {
    invokeCalls.length = 0;
    invokeImpl = async (cmd) => (cmd === 'check_permissions'
        ? { screen_recording: 'granted', microphone: 'granted' }
        : null);
    settingsManager.settings = {
        translation_mode: 'soniox',
        soniox_api_key: 'sk-test',
        audio_source: 'system',
        translation_type: 'one_way',
        source_language: 'auto',
        target_language: 'vi',
        early_suggestions: false,
    };
});

describe('session lifecycle', () => {
    test('stop() pauses without saving — no save_transcript call', async () => {
        const app = makeApp();
        app.isRunning = true;
        app.recordingStartTime = Date.now() - 5000;
        app.transcriptUI.addOriginal('hello', 'S1', 'en', 'system');

        await app.stop();

        expect(app.isRunning).toBe(false);
        expect(invokeCalls.map(c => c.cmd)).not.toContain('save_transcript');
        // Provisional-flush transcript stays in memory
        expect(app.transcriptUI.sessionLog.length).toBe(1);
    });

    test('paused draft renders every line; live capture shows only the tail', async () => {
        const app = makeApp();
        app._updateSessionChip = sessionMethods._updateSessionChip; // real wiring
        const ui = app.transcriptUI;
        app.isRunning = true;
        app.recordingStartTime = Date.now();

        // >90-char originals → one subtitle chunk per segment → 30 chunks total
        const pad = 'lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore';
        for (let i = 0; i < 30; i++) {
            ui.addOriginal(`${pad} ${i}`, 'S1', 'en', 'system');
            ui.addTranslation(`dịch ${i}`, 'system');
        }

        const pairCount = () =>
            (ui.contentEl.innerHTML.match(/class="subtitle-pair/g) || []).length;

        expect(ui.tailOnly).toBe(true);
        expect(pairCount()).toBeLessThanOrEqual(8); // MAX_CHUNKS tail

        await app.stop();

        expect(ui.tailOnly).toBe(false);
        expect(pairCount()).toBe(ui.sessionLog.length); // full draft visible
        expect(pairCount()).toBeGreaterThan(8);
    });

    test('opening a saved conversation renders the full transcript, not just the tail', async () => {
        const app = makeApp();
        app._updateSessionChip = sessionMethods._updateSessionChip; // real wiring

        const pad = 'lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore';
        const segs = [];
        for (let i = 0; i < 30; i++) {
            segs.push({
                id: i + 1,
                original: `${pad} ${i}`,
                translation: `dịch ${i}`,
                status: 'translated',
                speaker: '1',
                source: 'system',
                createdAt: 1791770984145 + i * 60_000, // 1min apart → many chunks
            });
        }
        // A segment that never got translated renders in place — not dropped,
        // not hoisted to the bottom as a fake "pending" block.
        segs.splice(5, 0, {
            id: 99, original: 'untranslated mid-session line', translation: null,
            status: 'original', speaker: '2', source: 'system',
            createdAt: 1791770984145 + 5 * 60_000 + 1000,
        });
        invokeImpl = async (cmd) => (cmd === 'read_transcript_segments'
            ? { version: 2, segments: segs }
            : null);

        await app._openConversation('2026-10-01_10-00-00.md');

        const pairCount =
            (app.transcriptUI.contentEl.innerHTML.match(/class="subtitle-pair/g) || []).length;

        expect(app.transcriptUI.tailOnly).toBe(false);
        expect(app.transcriptUI.segments.length).toBe(31); // display buffer not trimmed
        expect(pairCount).toBe(31);                        // every chunk rendered
    });

    test('+New saves dirty session then resets sessionLog', async () => {
        const app = makeApp();
        app.sessionActive = true;
        app.recordingStartTime = Date.now() - 10_000;
        app.sessionStartTime = new Date();
        app.transcriptUI.addOriginal('question one', 'S1', 'en', 'system');
        invokeImpl = async (cmd) => (cmd === 'save_transcript' ? '/tmp/2026-10-01_14-00-00.md' : null);

        await app._startNewSessionFlow();

        const save = invokeCalls.find(c => c.cmd === 'save_transcript');
        expect(save).toBeTruthy();
        expect(save.args.segments.segments.length).toBe(1); // v2 envelope
        expect(app.transcriptUI.sessionLog.length).toBe(0); // reset after save
        expect(app._sessionFilename).toBeNull();            // new session, new file later
    });

    test('save failure keeps the session alive', async () => {
        const app = makeApp();
        app.sessionActive = true;
        app.recordingStartTime = Date.now() - 10_000;
        app.transcriptUI.addOriginal('question one', 'S1', 'en', 'system');
        invokeImpl = async (cmd) => {
            if (cmd === 'save_transcript') throw new Error('disk full');
            return null;
        };

        await app._startNewSessionFlow();

        expect(app.transcriptUI.sessionLog.length).toBe(1); // not wiped
        expect(app._toasts.some(t => t.t === 'error')).toBe(true);
    });

    test('opening a saved conversation makes it resumable — ▶ records into the same file', async () => {
        const app = makeApp();
        invokeImpl = async (cmd) => {
            if (cmd === 'read_transcript_segments') {
                return { version: 2, segments: [{ id: 1, original: 'old q', translation: 'câu cũ', status: 'translated' }] };
            }
            if (cmd === 'check_permissions') return { screen_recording: 'granted', microphone: 'granted' };
            return null;
        };
        vi.spyOn(sonioxClient, 'connect').mockImplementation(() => {});

        await app._openConversation('2026-10-01_10-00-00.md');

        expect(app._sessionFilename).toBe('2026-10-01_10-00-00.md');
        expect(app.transcriptUI.sessionLog.length).toBe(1);
        expect(app._isSessionDirty()).toBe(false); // already persisted

        await app.start();
        expect(app.isRunning).toBe(true);
        vi.restoreAllMocks();
    });

    test('dirty draft is saved before a saved conversation loads', async () => {
        const app = makeApp();
        app.sessionActive = true;
        app.recordingStartTime = Date.now() - 10_000;
        app.transcriptUI.addOriginal('unsaved draft', 'S1', 'en', 'system');
        invokeImpl = async (cmd) => {
            if (cmd === 'save_transcript') return '/tmp/draft.md';
            if (cmd === 'read_transcript_segments') {
                return [{ id: 1, original: 'loaded q', translation: 'câu đã lưu', status: 'translated' }];
            }
            return null;
        };

        await app._openConversation('2026-10-01_11-00-00.md');

        // The live draft was finalized first, then replaced by the loaded file
        expect(invokeCalls.some(c => c.cmd === 'save_transcript')).toBe(true);
        expect(app._sessionFilename).toBe('2026-10-01_11-00-00.md');
        expect(app.transcriptUI.sessionLog.map(s => s.original)).toEqual(['loaded q']);
    });

    test('new segments after opening append and save back to the same file', async () => {
        const app = makeApp();
        invokeImpl = async (cmd) => {
            if (cmd === 'read_transcript_segments') {
                return [{ id: 1, original: 'old q', translation: 'câu cũ', status: 'translated' }];
            }
            if (cmd === 'save_transcript') return '/tmp/2026-10-01_10-00-00.md';
            return null;
        };
        await app._openConversation('2026-10-01_10-00-00.md');

        app.transcriptUI.addOriginal('new q', 'S1', 'en', 'system');
        const ok = await app._finalizeSession();

        expect(ok).toBe(true);
        const save = invokeCalls.find(c => c.cmd === 'save_transcript');
        expect(save.args.filename).toBe('2026-10-01_10-00-00.md'); // same file
        expect(save.args.segments.segments.length).toBe(2);       // old + new
    });

    test('Interview + both sources uses split capture', async () => {
        const app = makeApp();
        app.currentSource = 'both';
        app.currentTemplate = 'Interview';
        vi.spyOn(sonioxClient, 'connect').mockImplementation(() => {});
        vi.spyOn(sonioxMicClient, 'connect').mockImplementation(() => {});

        await app.start();

        expect(invokeCalls.map(c => c.cmd)).toContain('start_split_capture');
        expect(invokeCalls.map(c => c.cmd)).not.toContain('start_capture');
        expect(app.isRunning).toBe(true);
        vi.restoreAllMocks();
    });
});

describe('persistent error pill', () => {
    let pill;
    function makeToastHost() {
        pill = stubEl();
        pill._bound = false;
        let clickHandler = null;
        pill.addEventListener = (ev, fn) => { if (ev === 'click') clickHandler = fn; };
        pill._click = () => clickHandler?.();
        document.getElementById = (id) => (id === 'error-pill' ? pill : stubEl());
        document.querySelector = () => null;
        document.body = { appendChild() {}, classList: { add() {}, remove() {}, toggle() {} } };
        globalThis.requestAnimationFrame = (fn) => fn();
        return Object.assign({}, toastMethods, { _lastError: null });
    }

    test('error toast records _lastError and shows the pill; success does not', () => {
        const app = makeToastHost();
        app._showToast('LLM 401', 'error');
        expect(app._lastError.message).toBe('LLM 401');
        expect(pill.style.display).toBe('');
        app._showToast('ok', 'success');
        expect(app._lastError.message).toBe('LLM 401'); // unchanged
    });

    test('pill click dismisses without re-recording the error', () => {
        const app = makeToastHost();
        app._showToast('Mic lost', 'error');
        pill._click();
        expect(app._lastError).toBeNull();
        expect(pill.style.display).toBe('none');
    });

    test('new session clears a stale error', () => {
        const app = makeToastHost();
        app._showToast('boom', 'error');
        app._clearError();
        expect(app._lastError).toBeNull();
        expect(pill.style.display).toBe('none');
    });
});
