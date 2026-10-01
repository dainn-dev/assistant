import { describe, it, expect, beforeEach, vi } from 'vitest';
import { updaterMethods } from '../src/js/updater-ui.js';

function el() {
    return {
        style: {},
        classList: { add() {}, remove() {}, toggle() {}, contains: () => false },
        addEventListener() {},
        remove() {},
        textContent: '',
        hidden: false,
    };
}

let els;
globalThis.document = {
    getElementById: (id) => els[id] || (els[id] = el()),
    querySelector: () => null,
    querySelectorAll: () => [],
    createElement: () => el(),
    body: { appendChild() {} },
};

// updater.js singleton — drive its callbacks directly
const { updater } = await import('../src/js/updater.js');

function app() {
    return Object.assign({ _showToast: vi.fn() }, updaterMethods);
}

beforeEach(() => {
    els = {};
    updater.onError = null;
    updater.onCheckComplete = null;
    updater.onUpdateFound = null;
});

describe('update check surfacing', () => {
    it('startup check failure sets status but does not toast', () => {
        vi.useFakeTimers();
        const a = app();
        a._checkForUpdates();
        vi.runAllTimers();
        updater.onError(new Error('offline'));
        expect(document.getElementById('update-status-text').textContent).toContain('offline');
        expect(a._showToast).not.toHaveBeenCalled();
        vi.useRealTimers();
    });

    it('manual check failure shows the error pill toast', () => {
        vi.useFakeTimers();
        const a = app();
        a._checkForUpdates();
        vi.runAllTimers();
        a._triggerUpdateCheck();
        updater.onError(new Error('timeout'));
        expect(a._showToast).toHaveBeenCalledWith(expect.stringContaining('timeout'), 'error');
        vi.useRealTimers();
    });

    it('manual flag resets after check completes', () => {
        vi.useFakeTimers();
        const a = app();
        a._checkForUpdates();
        vi.runAllTimers();
        a._triggerUpdateCheck();
        updater.onCheckComplete(false);
        updater.onError(new Error('late'));
        expect(a._showToast).not.toHaveBeenCalled();
        vi.useRealTimers();
    });

    it('a failed check does not get overwritten with "up to date"', () => {
        vi.useFakeTimers();
        const a = app();
        a._checkForUpdates();
        vi.runAllTimers();
        // Plugin fires onError then onCheckComplete — error must survive
        updater.onError(new Error('Could not fetch a valid release JSON from the remote'));
        updater.onCheckComplete(false);
        expect(document.getElementById('update-status-text').textContent)
            .toContain('No release published yet');
        vi.useRealTimers();
    });
});
