import { describe, test, expect } from 'vitest';
import { sttProvider } from '../src/js/stt-providers.js';
import { sonioxClient, sonioxMicClient } from '../src/js/soniox.js';
import { deepgramClient, deepgramMicClient } from '../src/js/deepgram.js';

const llm = { llm_url: 'https://x/v1/chat/completions', llm_model: 'm', llm_api_key: 'k' };

describe('sttProvider', () => {
    test('maps each cloud mode to its own system/mic clients', () => {
        expect(sttProvider('soniox').clients).toEqual({ system: sonioxClient, mic: sonioxMicClient });
        expect(sttProvider('deepgram').clients).toEqual({ system: deepgramClient, mic: deepgramMicClient });
    });

    test('local and unknown modes have no cloud provider', () => {
        expect(sttProvider('local')).toBeNull();
        expect(sttProvider('bogus')).toBeNull();
    });

    test('soniox requires its API key', () => {
        expect(sttProvider('soniox').missingConfig({})).toMatch(/Soniox API key/);
        expect(sttProvider('soniox').missingConfig({ soniox_api_key: 'k' })).toBeNull();
    });

    test('deepgram requires its key and an LLM for translation', () => {
        const dg = sttProvider('deepgram');
        expect(dg.missingConfig({ ...llm })).toMatch(/Deepgram API key/);
        expect(dg.missingConfig({ deepgram_api_key: 'k' })).toMatch(/LLM/);
        expect(dg.missingConfig({ deepgram_api_key: 'k', ...llm })).toBeNull();
    });

    test('deepgram config carries model and language settings', () => {
        const cfg = sttProvider('deepgram').buildConfig({
            deepgram_api_key: 'k', deepgram_model: 'nova-2',
            translation_type: 'one_way', source_language: 'vi',
        });
        expect(cfg).toEqual({ apiKey: 'k', model: 'nova-2', translationType: 'one_way', sourceLanguage: 'vi' });
    });
});
