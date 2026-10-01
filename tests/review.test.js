import { describe, it, expect, beforeEach, vi } from 'vitest';
import { reviewMethods, formatScore } from '../src/js/review.js';

const invokeCalls = [];
let invokeImpl = async () => null;
globalThis.window = {
    __TAURI__: {
        core: {
            invoke: (cmd, args) => {
                invokeCalls.push({ cmd, args });
                return invokeImpl(cmd, args);
            },
            Channel: class {},
        },
    },
};

function el() {
    const e = {
        children: [],
        style: {},
        dataset: {},
        classList: { add() {}, remove() {}, toggle() {}, contains: () => false },
        addEventListener() {},
        appendChild(c) { e.children.push(c); return c; },
        textContent: '',
        innerHTML: '',
        value: '',
        disabled: false,
        hidden: false,
        setAttribute() {},
    };
    return e;
}

let els;
globalThis.document = {
    getElementById: (id) => els[id] || (els[id] = el()),
    createElement: () => el(),
    querySelectorAll: () => [],
    activeElement: null,
};
globalThis.confirm = vi.fn(() => true);

const REVIEW = {
    version: 1,
    generated_at: '2026-10-01T00:00:00Z',
    model: 'gpt-4o-mini',
    roles_known: true,
    overall: 'Solid answers, needs metrics.',
    questions: [
        {
            question: 'Deadlock handling?',
            answer_summary: 'Ordered locks, retries',
            score: 4,
            feedback: 'Good depth, add metrics',
            stronger_answer: 'Mention detection + prevention split',
        },
        { question: 'Team conflict?', answer_summary: '', score: null, feedback: 'Vague', stronger_answer: '' },
    ],
    practice: ['Tell me about a failure'],
};

function app() {
    return Object.assign(
        {
            _getInterviewUserId: () => 'u1',
            _showToast: vi.fn(),
            _loadConversationList: vi.fn(async () => {}),
            _suggestionsDock: { docked: false },
            activeConversationFilename: '2026-10-01_10-00-00.md',
        },
        reviewMethods
    );
}

beforeEach(() => {
    els = {};
    invokeCalls.length = 0;
    invokeImpl = async () => null;
});

describe('formatScore', () => {
    it('renders dots and em-dash for null', () => {
        expect(formatScore(4)).toBe('●●●●○');
        expect(formatScore(null)).toBe('—');
        expect(formatScore(7)).toBe('●●●●●');
    });
});

describe('review flow', () => {
    it('renders cached review without invoking review_session', async () => {
        invokeImpl = async () => REVIEW;
        const a = app();
        await a._reviewLoadCached('x.md');
        expect(a._review).toBe(REVIEW);
        expect(invokeCalls.map((c) => c.cmd)).toEqual(['read_session_review']);
        expect(document.getElementById('review-questions').children).toHaveLength(2);
        expect(document.getElementById('review-panel').hidden).toBe(false);
    });

    it('Review button on unreviewed session confirms then invokes review_session', async () => {
        invokeImpl = async (cmd) => (cmd === 'review_session' ? REVIEW : null);
        const a = app();
        a._reviewTranscriptKb = 42;
        await a._reviewGenerate('x.md', false);
        expect(confirm).toHaveBeenCalled();
        const call = invokeCalls.find((c) => c.cmd === 'review_session');
        expect(call.args).toEqual({ filename: 'x.md', force: false });
        expect(a._loadConversationList).toHaveBeenCalled();
    });

    it('null scores render em-dash', async () => {
        invokeImpl = async () => REVIEW;
        const a = app();
        await a._reviewLoadCached('x.md');
        const scores = document.getElementById('review-questions')
            .children.map((li) => li.children.find((c) => c.className === 'review-meta'))
            .map((m) => m.children[0].textContent);
        expect(scores).toEqual(['●●●●○', '—']);
    });

    it('_reviewHide hides panel and restores right panel when nothing docked', async () => {
        invokeImpl = async () => REVIEW;
        const a = app();
        await a._reviewLoadCached('x.md');
        a._reviewHide();
        expect(document.getElementById('review-panel').hidden).toBe(true);
        expect(document.getElementById('right-panel').style.display).toBe('none');
    });
});
