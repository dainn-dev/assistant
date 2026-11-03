/**
 * Cloud speech-to-text providers keyed by `translation_mode`. Each has a
 * system (primary) and mic client sharing the SonioxClient callback surface.
 */

import { sonioxClient, sonioxMicClient } from './soniox.js';
import { deepgramClient, deepgramMicClient } from './deepgram.js';

const PROVIDERS = {
    soniox: {
        label: 'Soniox Cloud API',
        clients: { system: sonioxClient, mic: sonioxMicClient },
        missingConfig: (s) => (s.soniox_api_key ? null : 'Soniox API key is required. Add it in Settings.'),
        buildConfig: (s) => ({
            apiKey: s.soniox_api_key,
            sourceLanguage: s.source_language,
            targetLanguage: s.target_language,
            customContext: s.custom_context,
            translationType: s.translation_type || 'one_way',
            languageA: s.language_a,
            languageB: s.language_b,
            languageHintsStrict: s.language_hints_strict || false,
            endpointDelay: s.endpoint_delay || 3000,
        }),
    },
    deepgram: {
        label: 'Deepgram + LLM',
        clients: { system: deepgramClient, mic: deepgramMicClient },
        missingConfig: (s) => {
            if (!s.deepgram_api_key) return 'Deepgram API key is required. Add it in Settings.';
            if (!s.llm_url || !s.llm_model || !s.llm_api_key) {
                return 'Deepgram translates through the LLM — configure it in Settings → AI.';
            }
            return null;
        },
        buildConfig: (s) => ({
            apiKey: s.deepgram_api_key,
            model: s.deepgram_model || 'nova-3',
            translationType: s.translation_type || 'one_way',
            sourceLanguage: s.source_language,
        }),
    },
};

/** @param {string} mode @returns {object|null} null for 'local' / unknown modes */
export function sttProvider(mode) {
    return PROVIDERS[mode] || null;
}
