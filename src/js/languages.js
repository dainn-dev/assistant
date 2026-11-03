/**
 * Language names — single source for the settings selects and the dual-view
 * panel headers.
 */

export const LANG_NAMES = {
    vi: 'Vietnamese', en: 'English', ja: 'Japanese', ko: 'Korean', zh: 'Chinese',
    fr: 'French', de: 'German', es: 'Spanish', th: 'Thai', id: 'Indonesian',
    af: 'Afrikaans', sq: 'Albanian', ar: 'Arabic', az: 'Azerbaijani', eu: 'Basque',
    be: 'Belarusian', bn: 'Bengali', bs: 'Bosnian', bg: 'Bulgarian', ca: 'Catalan',
    hr: 'Croatian', cs: 'Czech', da: 'Danish', nl: 'Dutch', et: 'Estonian',
    fi: 'Finnish', gl: 'Galician', el: 'Greek', gu: 'Gujarati', he: 'Hebrew',
    hi: 'Hindi', hu: 'Hungarian', it: 'Italian', kn: 'Kannada', kk: 'Kazakh',
    lv: 'Latvian', lt: 'Lithuanian', mk: 'Macedonian', ms: 'Malay', ml: 'Malayalam',
    mr: 'Marathi', no: 'Norwegian', fa: 'Persian', pl: 'Polish', pt: 'Portuguese',
    pa: 'Punjabi', ro: 'Romanian', ru: 'Russian', sr: 'Serbian', sk: 'Slovak',
    sl: 'Slovenian', sw: 'Swahili', sv: 'Swedish', tl: 'Tagalog', ta: 'Tamil',
    te: 'Telugu', tr: 'Turkish', uk: 'Ukrainian', ur: 'Urdu', cy: 'Welsh',
};

/** @param {string} code @returns {string} */
export function languageName(code) {
    return LANG_NAMES[code] || String(code || '').toUpperCase();
}

/**
 * Dual-view column headers for the current session. Two-way sessions put
 * both languages in each column, so only one-way gets concrete names.
 * @param {{ mode?: string, sourceLang?: string, targetLang?: string }} session
 * @returns {{ source: string, target: string }}
 */
export function dualPanelHeaders({ mode, sourceLang, targetLang } = {}) {
    if (mode !== 'one_way' || !targetLang) {
        return { source: 'Original', target: 'Translation' };
    }
    return {
        source: !sourceLang || sourceLang === 'auto' ? 'Original' : languageName(sourceLang),
        target: languageName(targetLang),
    };
}
