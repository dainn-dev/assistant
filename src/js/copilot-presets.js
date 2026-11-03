/**
 * Copilot presets — UI-side labels and feature flags. Prompts and the
 * matching document slots live in Rust (src-tauri/src/copilot/presets.rs);
 * keep `mode` values in sync with `preset_for_mode` there.
 */

const PRESETS = {
    interview: {
        id: 'interview',
        mode: 'Interview', // value stored in settings.app_mode
        label: 'Interview',
        description: 'answer suggestions grounded in your CV / JD + profile',
        // Upload slots: first file → slot a, second → slot b. docTypes are the
        // stored doc_type of each slot (Rust `doc_slots`).
        uploads: { a: 'CV', b: 'JD' },
        docTypes: ['cv', 'jd'],
        hasProfile: true,
        panelTitle: 'Suggested answers',
        emptyText: 'Waiting for interviewer question…',
    },
    meeting: {
        id: 'meeting',
        mode: 'Meeting',
        label: 'Meeting',
        description: 'what to say next + minutes, grounded in your brief / agenda',
        uploads: { a: 'Brief', b: 'Agenda' },
        docTypes: ['brief', 'agenda'],
        hasProfile: false,
        panelTitle: 'What to say next',
        emptyText: 'Listening for questions directed at you…',
    },
};

/** Presets in dropdown order. */
export const COPILOT_PRESETS = Object.values(PRESETS);

/** @param {string|null} appMode @returns {object|null} null = plain translator */
export function copilotPreset(appMode) {
    return PRESETS[String(appMode || '').trim().toLowerCase()] || null;
}

/**
 * Whether files picked under `uploadsPresetId` may stay picked when the
 * active preset becomes `nextPresetId` — a CV must never be re-ingested as
 * a meeting brief. Turning the copilot off (null) keeps them.
 */
export function pickedUploadsSurvive(uploadsPresetId, nextPresetId) {
    return !uploadsPresetId || !nextPresetId || uploadsPresetId === nextPresetId;
}

/** Canonical `app_mode` to store: the preset's mode name, or '' for none. */
export function normalizeAppMode(value) {
    return copilotPreset(value)?.mode || '';
}
