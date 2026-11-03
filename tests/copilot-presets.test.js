import { describe, test, expect } from 'vitest';
import {
    copilotPreset, normalizeAppMode, pickedUploadsSurvive, COPILOT_PRESETS,
} from '../src/js/copilot-presets.js';

describe('copilotPreset', () => {
    test('resolves stored app modes case-insensitively', () => {
        expect(copilotPreset('Interview').id).toBe('interview');
        expect(copilotPreset('meeting').id).toBe('meeting');
    });

    test('plain translator and unknown modes have no copilot', () => {
        expect(copilotPreset('')).toBeNull();
        expect(copilotPreset(null)).toBeNull();
        expect(copilotPreset('bogus')).toBeNull();
    });

    test('meeting has its own upload labels and no profile', () => {
        const m = copilotPreset('Meeting');
        expect(m.uploads).toEqual({ a: 'Brief', b: 'Agenda' });
        expect(m.hasProfile).toBe(false);
        expect(copilotPreset('Interview').hasProfile).toBe(true);
    });

    test('every preset is listed for the mode dropdown', () => {
        expect(COPILOT_PRESETS.map((p) => p.mode)).toEqual(['Interview', 'Meeting']);
    });
});

describe('pickedUploadsSurvive', () => {
    test('picked files never carry over into another preset', () => {
        expect(pickedUploadsSurvive('interview', 'meeting')).toBe(false);
        expect(pickedUploadsSurvive('interview', 'interview')).toBe(true);
        // Turning the copilot off keeps them; nothing picked yet keeps "nothing".
        expect(pickedUploadsSurvive('interview', null)).toBe(true);
        expect(pickedUploadsSurvive(null, 'meeting')).toBe(true);
    });
});

describe('normalizeAppMode', () => {
    test('stores the canonical mode name, or empty for the plain translator', () => {
        expect(normalizeAppMode('meeting')).toBe('Meeting');
        expect(normalizeAppMode('Interview')).toBe('Interview');
        expect(normalizeAppMode('nope')).toBe('');
        expect(normalizeAppMode('')).toBe('');
    });
});
