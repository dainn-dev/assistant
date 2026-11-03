import { describe, test, expect, vi } from 'vitest';
import { DeepgramSegmenter, deepgramQuery, DeepgramClient } from '../src/js/deepgram.js';

const results = (transcript, { isFinal = false, speechFinal = false, languages, confidence = 0.9 } = {}) => ({
    type: 'Results',
    is_final: isFinal,
    speech_final: speechFinal,
    channel: { alternatives: [{ transcript, confidence, languages }] },
});

describe('DeepgramSegmenter', () => {
    test('interim results only update the provisional text', () => {
        const seg = new DeepgramSegmenter();
        expect(seg.handle(results('hello wor'))).toEqual({
            provisional: 'hello wor', finalized: null, confidence: null,
        });
    });

    test('final fragments accumulate until speech_final closes the utterance', () => {
        const seg = new DeepgramSegmenter();
        const first = seg.handle(results('Hello there,', { isFinal: true, languages: ['en'] }));
        expect(first.finalized).toBeNull();
        expect(first.provisional).toBe('Hello there,');
        expect(seg.handle(results('how are', {})).provisional).toBe('Hello there, how are');

        const done = seg.handle(results('how are you?', { isFinal: true, speechFinal: true }));
        expect(done.finalized).toEqual({ text: 'Hello there, how are you?', language: 'en' });
        expect(done.provisional).toBe('');
    });

    test('UtteranceEnd flushes a pending buffer, and is a no-op when empty', () => {
        const seg = new DeepgramSegmenter();
        seg.handle(results('trailing words', { isFinal: true }));
        expect(seg.handle({ type: 'UtteranceEnd' }).finalized).toEqual({ text: 'trailing words', language: null });
        expect(seg.handle({ type: 'UtteranceEnd' }).finalized).toBeNull();
    });

    test('CJK fragments are joined without inserted spaces', () => {
        const seg = new DeepgramSegmenter();
        seg.handle(results('こんにちは', { isFinal: true }));
        const out = seg.handle(results('世界', { isFinal: true, speechFinal: true }));
        expect(out.finalized.text).toBe('こんにちは世界');
    });

    test('an overlong buffer is flushed even without speech_final', () => {
        const seg = new DeepgramSegmenter({ maxChars: 10 });
        const out = seg.handle(results('a long running sentence', { isFinal: true }));
        expect(out.finalized.text).toBe('a long running sentence');
    });

    test('final results report confidence', () => {
        const seg = new DeepgramSegmenter();
        expect(seg.handle(results('ok', { isFinal: true, confidence: 0.75 })).confidence).toBe(0.75);
    });
});

describe('deepgramQuery', () => {
    test('auto source uses multilingual mode with short endpointing', () => {
        const q = new URLSearchParams(deepgramQuery({ translationType: 'one_way', sourceLanguage: 'auto' }));
        expect(q.get('language')).toBe('multi');
        expect(q.get('endpointing')).toBe('100');
        expect(q.get('model')).toBe('nova-3');
        expect(q.get('encoding')).toBe('linear16');
        expect(q.get('sample_rate')).toBe('16000');
        expect(q.get('interim_results')).toBe('true');
    });

    test('two-way always uses multilingual mode', () => {
        const q = new URLSearchParams(deepgramQuery({ translationType: 'two_way', sourceLanguage: 'ja' }));
        expect(q.get('language')).toBe('multi');
    });

    test('an explicit source language and model are passed through', () => {
        const q = new URLSearchParams(deepgramQuery({
            translationType: 'one_way', sourceLanguage: 'vi', model: 'nova-2',
        }));
        expect(q.get('language')).toBe('vi');
        expect(q.get('model')).toBe('nova-2');
    });
});

describe('DeepgramClient translations', () => {
    test('translations are emitted in utterance order even if responses arrive out of order', async () => {
        const resolvers = {};
        window.__TAURI__.core.invoke = vi.fn((cmd, { text }) =>
            new Promise((resolve) => { resolvers[text] = resolve; }));
        const client = new DeepgramClient();
        const emitted = [];
        client.onTranslation = (t) => emitted.push(t);

        client._emitFinal({ text: 'one', language: null });
        client._emitFinal({ text: 'two', language: null });
        resolvers.two('hai');
        resolvers.one('một');
        await client._translateTail;

        expect(emitted).toEqual(['một', 'hai']);
    });

    test('a failed translation reports an error and keeps later ones flowing', async () => {
        window.__TAURI__.core.invoke = vi.fn(async (cmd, { text }) => {
            if (text === 'bad') throw 'LLM down';
            return `vi:${text}`;
        });
        const client = new DeepgramClient();
        const emitted = [];
        const errors = [];
        client.onTranslation = (t) => emitted.push(t);
        client.onError = (e) => errors.push(e);

        client._emitFinal({ text: 'bad', language: null });
        client._emitFinal({ text: 'good', language: null });
        await client._translateTail;

        expect(errors).toEqual(['Translation failed: LLM down']);
        expect(emitted).toEqual(['⚠', 'vi:good']);
    });
});
