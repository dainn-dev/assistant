import { describe, test, expect } from 'vitest';
import { languageName, dualPanelHeaders } from '../src/js/languages.js';

describe('languageName', () => {
    test('maps known codes to English names', () => {
        expect(languageName('ja')).toBe('Japanese');
        expect(languageName('vi')).toBe('Vietnamese');
    });

    test('falls back to the uppercased code for unknown languages', () => {
        expect(languageName('xx')).toBe('XX');
    });
});

describe('dualPanelHeaders', () => {
    test('one-way uses the session source and target languages', () => {
        expect(dualPanelHeaders({ mode: 'one_way', sourceLang: 'ja', targetLang: 'vi' }))
            .toEqual({ source: 'Japanese', target: 'Vietnamese' });
    });

    test('one-way with auto-detected source labels the source column generically', () => {
        expect(dualPanelHeaders({ mode: 'one_way', sourceLang: 'auto', targetLang: 'en' }))
            .toEqual({ source: 'Original', target: 'English' });
    });

    test('two-way mixes both languages in each column, so headers stay generic', () => {
        expect(dualPanelHeaders({ mode: 'two_way', sourceLang: 'ja', targetLang: 'vi' }))
            .toEqual({ source: 'Original', target: 'Translation' });
    });

    test('missing session info yields generic headers', () => {
        expect(dualPanelHeaders({})).toEqual({ source: 'Original', target: 'Translation' });
    });
});
