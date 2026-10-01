import { describe, it, expect, beforeEach, vi } from 'vitest';
import { profileMethods, mergeDraft } from '../src/js/profile.js';

const invokeCalls = [];
let invokeImpl = async () => [];
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
let profileTab;
globalThis.document = {
    getElementById: (id) => els[id] || (els[id] = el()),
    createElement: () => el(),
    querySelector: (sel) => (sel.includes('tab-profile') ? profileTab : null),
    querySelectorAll: () => [],
    activeElement: null,
};

function app() {
    const a = {
        _getInterviewUserId: () => 'u1',
        _showToast: vi.fn(),
    };
    return Object.assign(a, profileMethods);
}

beforeEach(() => {
    els = {};
    profileTab = el();
    invokeCalls.length = 0;
    invokeImpl = async () => [];
});

describe('mergeDraft', () => {
    it('replaces summary and dedupes stories by title', () => {
        const existing = [
            { id: 's0', kind: 'summary', title: '', content: 'old summary' },
            { id: 'st1', kind: 'story', title: 'Deadlock fix', content: 'a' },
        ];
        const draft = [
            { id: 'd0', kind: 'summary', title: '', content: 'new summary' },
            { id: 'd1', kind: 'story', title: 'Deadlock fix', content: 'dup — dropped' },
            { id: 'd2', kind: 'story', title: 'Scaling kafka', content: 'b' },
        ];
        const out = mergeDraft(existing, draft);
        const summary = out.find((i) => i.kind === 'summary');
        expect(summary.id).toBe('d0');
        expect(summary._draft).toBe(true);
        const titles = out.filter((i) => i.kind === 'story').map((i) => i.title);
        expect(titles).toEqual(['Scaling kafka', 'Deadlock fix']);
    });

    it('keeps existing summary when draft has none', () => {
        const out = mergeDraft(
            [{ id: 's0', kind: 'summary', title: '', content: 'old' }],
            [{ id: 'd1', kind: 'strength', title: 'Rust', content: 'x' }]
        );
        expect(out[0].id).toBe('s0');
        expect(out).toHaveLength(2);
    });
});

describe('profile render + save', () => {
    it('render shows empty state when no items', () => {
        const a = app();
        a._profileItems = [];
        a._profileRender();
        const list = document.getElementById('profile-items');
        expect(list.children[0].className).toContain('profile-empty');
    });

    it('typing in a story saves once after debounce', async () => {
        vi.useFakeTimers();
        const a = app();
        a._profileItems = [];
        const item = { id: 's1', kind: 'story', title: 'T', content: 'C' };
        a._profileQueueSave(item);
        a._profileQueueSave({ ...item, content: 'C2' });
        await vi.advanceTimersByTimeAsync(700);
        expect(invokeCalls.filter((c) => c.cmd === 'save_profile_item')).toHaveLength(1);
        expect(invokeCalls[0].args.item.content).toBe('C2');
        expect(invokeCalls[0].args.item.user_id).toBe('u1');
        vi.useRealTimers();
    });

    it('draft error shows message and saves nothing', async () => {
        invokeImpl = async () => { throw 'Upload a CV first'; };
        const a = app();
        a._profileItems = [];
        await a._profileDraft();
        expect(document.getElementById('profile-status').textContent).toContain('Upload a CV first');
        expect(invokeCalls.filter((c) => c.cmd === 'save_profile_item')).toHaveLength(0);
    });

    it('Profile tab hidden unless Interview mode', () => {
        const a = app();
        a.currentTemplate = null;
        a._profileSyncTabVisibility();
        expect(profileTab.style.display).toBe('none');
        a.currentTemplate = 'Interview';
        a._profileSyncTabVisibility();
        expect(profileTab.style.display).toBe('');
    });

    it('accept saves draft items and clears draft flag', async () => {
        vi.useFakeTimers();
        const a = app();
        a._profileItems = [
            { id: 'd1', kind: 'story', title: 'T', content: 'C', _draft: true },
        ];
        a._profilePreDraftItems = [];
        a._profileAcceptDraft();
        await vi.advanceTimersByTimeAsync(700);
        expect(invokeCalls.filter((c) => c.cmd === 'save_profile_item')).toHaveLength(1);
        expect(a._profileItems[0]._draft).toBe(false);
        vi.useRealTimers();
    });
});
