/**
 * Deepgram streaming STT client — same callback surface as SonioxClient
 * (onOriginal / onTranslation / onProvisional / onStatusChange / onError /
 * onConfidence) so the app can swap providers per session.
 *
 * Deepgram only transcribes: each finished utterance is translated by the
 * configured LLM through the `translate_segment` Rust command.
 */

import { invoke } from './ipc.js';

const DEEPGRAM_ENDPOINT = 'wss://api.deepgram.com/v1/listen';
const KEEPALIVE_MS = 5000;
const MAX_RECONNECT = 3;
const RECONNECT_DELAY_MS = 1000;
// Translating at every `is_final` fragment would cut sentences mid-way;
// utterances are flushed on speech_final / UtteranceEnd, or at this length.
const MAX_UTTERANCE_CHARS = 400;
// Scripts written without spaces between words (kana, CJK ideographs,
// full-width forms). Hangul is excluded — Korean separates words with spaces.
const NO_SPACE_SCRIPT = /[぀-ヿ㐀-鿿＀-￯]/;

function joinFragments(a, b) {
    if (!a) return b;
    if (!b) return a;
    return NO_SPACE_SCRIPT.test(a.slice(-1)) || NO_SPACE_SCRIPT.test(b[0])
        ? a + b
        : `${a} ${b}`;
}

/**
 * Query string for the /v1/listen WebSocket. `language=multi` is nova-3's
 * code-switching mode — the only way to stream without a fixed language,
 * so it serves both auto-detect and two-way sessions.
 * @param {{ model?: string, translationType?: string, sourceLanguage?: string }} config
 * @returns {string}
 */
export function deepgramQuery({ model, translationType, sourceLanguage } = {}) {
    const multi = translationType === 'two_way' || !sourceLanguage || sourceLanguage === 'auto';
    return new URLSearchParams({
        model: model || 'nova-3',
        language: multi ? 'multi' : sourceLanguage,
        // Deepgram recommends 100 ms endpointing for code-switching.
        endpointing: multi ? '100' : '300',
        encoding: 'linear16',
        sample_rate: '16000',
        channels: '1',
        interim_results: 'true',
        utterance_end_ms: '1000',
        smart_format: 'true',
        punctuate: 'true',
    }).toString();
}

/**
 * Turns Deepgram messages into provisional text and finished utterances.
 * `provisional: null` means "no change"; `''` clears it.
 */
export class DeepgramSegmenter {
    constructor({ maxChars = MAX_UTTERANCE_CHARS } = {}) {
        this.maxChars = maxChars;
        this.reset();
    }

    reset() {
        this._buffer = '';
        this._language = null;
    }

    /** Emit and clear the pending utterance, if any. */
    flush() {
        if (!this._buffer) return null;
        const finalized = { text: this._buffer, language: this._language };
        this.reset();
        return finalized;
    }

    handle(msg) {
        if (msg?.type === 'UtteranceEnd') {
            const finalized = this.flush();
            return { provisional: finalized ? '' : null, finalized, confidence: null };
        }
        if (msg?.type !== 'Results') {
            return { provisional: null, finalized: null, confidence: null };
        }

        const alt = msg.channel?.alternatives?.[0] || {};
        const transcript = (alt.transcript || '').trim();
        if (!msg.is_final) {
            return { provisional: joinFragments(this._buffer, transcript), finalized: null, confidence: null };
        }

        if (transcript) {
            this._buffer = joinFragments(this._buffer, transcript);
            if (alt.languages?.[0]) this._language = alt.languages[0];
        }
        const confidence = transcript && typeof alt.confidence === 'number' ? alt.confidence : null;
        if (msg.speech_final || this._buffer.length >= this.maxChars) {
            const finalized = this.flush();
            if (finalized) return { provisional: '', finalized, confidence };
        }
        return { provisional: this._buffer, finalized: null, confidence };
    }
}

export class DeepgramClient {
    constructor() {
        this.ws = null;
        this._config = null;
        this._intentionalClose = false;
        this._opened = false;
        this._keepaliveTimer = null;
        this._reconnectAttempts = 0;
        this._segmenter = new DeepgramSegmenter();
        // Translations run in parallel but are emitted in utterance order —
        // the transcript UI pairs each translation with the oldest original.
        this._translateTail = Promise.resolve();

        this.onOriginal = null;       // (text, speaker, language) => {}
        this.onTranslation = null;    // (text) => {}
        this.onProvisional = null;    // (text, speaker, language) => {}
        this.onStatusChange = null;   // (status) => {}
        this.onError = null;          // (error) => {}
        this.onConfidence = null;     // (avgConfidence) => {}
    }

    connect(config) {
        if (!config?.apiKey) {
            this.onStatusChange?.('error');
            this.onError?.('Deepgram API key is required. Please add it in Settings.');
            return;
        }
        this._config = config;
        this._intentionalClose = false;
        this._reconnectAttempts = 0;
        this._segmenter.reset();
        this._open();
    }

    _open() {
        this.onStatusChange?.('connecting');
        this._opened = false;
        let ws;
        try {
            // Browsers can't set headers on a WebSocket; Deepgram accepts the
            // key as the second subprotocol of ['token', key].
            ws = new WebSocket(`${DEEPGRAM_ENDPOINT}?${deepgramQuery(this._config)}`, ['token', this._config.apiKey]);
        } catch (err) {
            this.onStatusChange?.('error');
            this.onError?.(`Failed to create WebSocket: ${err.message}`);
            return;
        }
        ws.binaryType = 'arraybuffer';
        ws.onopen = () => {
            this._opened = true;
            this._reconnectAttempts = 0;
            this._startKeepalive();
            this.onStatusChange?.('connected');
        };
        ws.onmessage = (event) => this._onMessage(event.data);
        ws.onclose = (event) => this._onClose(ws, event);
        this.ws = ws;
    }

    _onMessage(data) {
        if (typeof data !== 'string') return;
        let msg;
        try {
            msg = JSON.parse(data);
        } catch {
            return; // Deepgram only sends JSON text frames; ignore anything else
        }
        const out = this._segmenter.handle(msg);
        if (out.confidence !== null) this.onConfidence?.(out.confidence);
        if (out.finalized) this._emitFinal(out.finalized);
        if (out.provisional !== null) this.onProvisional?.(out.provisional, null, null);
    }

    _onClose(ws, event) {
        if (ws !== this.ws) return; // a replaced socket closing late
        this._stopKeepalive();
        if (this._intentionalClose) return;

        const reason = event.reason ? `: ${event.reason}` : '';
        if (!this._opened) {
            this.onStatusChange?.('error');
            this.onError?.(`Deepgram refused the connection (code ${event.code}${reason}). Check the API key, model and language in Settings.`);
            return;
        }
        if (this._reconnectAttempts >= MAX_RECONNECT) {
            this.onStatusChange?.('error');
            this.onError?.(`Deepgram connection lost${reason}. Reconnect failed after ${MAX_RECONNECT} attempts.`);
            return;
        }
        this._reconnectAttempts++;
        const pending = this._segmenter.flush();
        if (pending) this._emitFinal(pending);
        this.onError?.(`Deepgram connection lost${reason}. Reconnecting (${this._reconnectAttempts}/${MAX_RECONNECT})...`);
        setTimeout(() => {
            if (!this._intentionalClose) this._open();
        }, RECONNECT_DELAY_MS * this._reconnectAttempts);
    }

    _emitFinal({ text, language }) {
        this.onOriginal?.(text, null, language);
        const result = invoke('translate_segment', { text }).then(
            (translation) => ({ ok: true, translation }),
            (error) => ({ ok: false, error }),
        );
        this._translateTail = this._translateTail
            .then(() => result)
            .then((r) => {
                if (r.ok) {
                    this.onTranslation?.(r.translation);
                } else {
                    this.onError?.(`Translation failed: ${r.error}`);
                    this.onTranslation?.('⚠');
                }
            });
    }

    sendAudio(pcmData) {
        if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(pcmData);
    }

    disconnect() {
        this._intentionalClose = true;
        this._stopKeepalive();
        // Translate whatever was still being spoken when the user stopped.
        const pending = this._segmenter.flush();
        if (pending) this._emitFinal(pending);
        if (this.ws) {
            try {
                if (this.ws.readyState === WebSocket.OPEN) {
                    this.ws.send(JSON.stringify({ type: 'CloseStream' }));
                }
                this.ws.close(1000);
            } catch {
                // already closing — nothing to clean up
            }
            this.ws = null;
        }
        this.onStatusChange?.('disconnected');
    }

    _startKeepalive() {
        this._stopKeepalive();
        this._keepaliveTimer = setInterval(() => {
            if (this.ws?.readyState === WebSocket.OPEN) {
                this.ws.send(JSON.stringify({ type: 'KeepAlive' }));
            }
        }, KEEPALIVE_MS);
    }

    _stopKeepalive() {
        if (this._keepaliveTimer) clearInterval(this._keepaliveTimer);
        this._keepaliveTimer = null;
    }
}

export const deepgramClient = new DeepgramClient();
export const deepgramMicClient = new DeepgramClient();
