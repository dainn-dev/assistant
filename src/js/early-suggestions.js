// Early suggestions — generate answer hints while the interviewer is still
// speaking. A pure scheduler (testable without DOM) decides when a partial
// question is stable enough to send; the App mixin wires it to Tauri events.
//
// Budget: 1 in-flight request, 1 pending revision, ≤3 speculative starts per
// interviewer turn, ≤20 starts per rolling minute, ≥1.5s between starts.

import { settingsManager } from './settings.js';

const { invoke } = window.__TAURI__.core;

export function normalizeSnapshot(text) {
    return String(text || '').replace(/\s+/g, ' ').trim().toLowerCase();
}

function _wordCount(text) {
    return text.split(' ').filter(Boolean).length;
}
function _cjkCount(text) {
    return (text.match(/[一-鿿぀-ヿ가-힯]/g) || []).length;
}
function _isMeaningful(text) {
    return _wordCount(text) >= 4 || _cjkCount(text) >= 12;
}

export class EarlyScheduler {
    constructor({ now = () => Date.now(), fire = () => 0, cancel = () => {} } = {}) {
        this._now = now;
        this._fire = fire;
        this._cancel = cancel;

        this.epoch = 0;
        this.turnId = 0;
        this.revision = 0;
        this.inFlightId = null;
        this.pendingSnapshot = null;
        this._lastSnapshot = '';
        this._stableCount = 0;
        this._lastFireAt = -Infinity;
        this._turnFires = 0;
        this._minuteFires = [];   // timestamps of fires in the last 60s
        this._lastFiredSnapshot = '';
    }

    static TICK_MS = 600;
    static MIN_SPACING_MS = 1500;
    static MAX_PER_TURN = 3;
    static MAX_PER_MINUTE = 20;
    static STABLE_OBSERVATIONS = 2;

    /** Feed the latest transcript snapshot. source: 'system' | 'mic' */
    observe(snapshot, source) {
        if (source !== 'system') return;
        const norm = normalizeSnapshot(snapshot);
        if (!norm) return;

        if (norm !== this._lastSnapshot) {
            // Correction/shrinkage or growth — restarts stability counting.
            if (this._lastSnapshot && !norm.startsWith(this._lastSnapshot)) {
                // Meaning changed mid-question — in-flight answer is stale.
                this.revision++;
                if (this.inFlightId != null) {
                    this._cancel(this.inFlightId);
                    this.inFlightId = null;
                }
                this.pendingSnapshot = null;
            }
            this._lastSnapshot = norm;
            this._stableCount = 1;
            return;
        }

        this._stableCount++;
        if (this._stableCount < EarlyScheduler.STABLE_OBSERVATIONS) return;
        this._maybeFire(norm);
    }

    /** Interviewer finished a turn (endpoint detection / finalized segment). */
    endpoint(source) {
        if (source !== 'system') return;
        const norm = this._lastSnapshot;
        // One final fire if the last stable text was never sent.
        if (norm && norm !== this._lastFiredSnapshot) {
            this._maybeFire(norm, { final: true });
        }
        // Next system speech starts a new turn.
        this.turnId++;
        this.revision = 0;
        this._turnFires = 0;
        this._lastSnapshot = '';
        this._stableCount = 0;
        this._lastFiredSnapshot = '';
        this.pendingSnapshot = null;
    }

    _maybeFire(norm, { final = false } = {}) {
        if (!_isMeaningful(norm)) return;
        if (this._turnFires >= EarlyScheduler.MAX_PER_TURN && !final) return;

        const now = this._now();
        const cutoff = now - 60_000;
        this._minuteFires = this._minuteFires.filter((t) => t > cutoff);
        if (this._minuteFires.length >= EarlyScheduler.MAX_PER_MINUTE) return;
        if (now - this._lastFireAt < EarlyScheduler.MIN_SPACING_MS) {
            // Queue as pending instead — fires when spacing allows or in-flight ends.
            if (norm !== this._lastFiredSnapshot) this.pendingSnapshot = norm;
            return;
        }

        if (this.inFlightId != null) {
            if (norm !== this._lastFiredSnapshot) this.pendingSnapshot = norm;
            return;
        }

        this._doFire(norm, now);
    }

    _doFire(norm, now) {
        this._lastFireAt = now;
        this._turnFires++;
        this._minuteFires.push(now);
        this._lastFiredSnapshot = norm;
        this.pendingSnapshot = null;
        this.inFlightId = this._fire(norm, {
            epoch: this.epoch,
            turnId: this.turnId,
            revision: this.revision,
        });
    }

    /** Mark a request finished; fires the pending snapshot if it's still relevant. */
    requestDone(requestId) {
        if (requestId !== this.inFlightId) return; // stale completion
        this.inFlightId = null;
        if (this.pendingSnapshot && this.pendingSnapshot !== this._lastFiredSnapshot) {
            const snap = this.pendingSnapshot;
            this.pendingSnapshot = null;
            this._maybeFire(snap);
        }
    }

    /** Bump epoch — all in-flight results are stale after this. */
    bumpEpoch() {
        this.epoch++;
        if (this.inFlightId != null) {
            this._cancel(this.inFlightId);
            this.inFlightId = null;
        }
        this.pendingSnapshot = null;
    }
}

export const earlySuggestionMethods = {

    _earlySuggestionsEligible() {
        const s = settingsManager.get();
        return s.early_suggestions === true
            && this.translationMode === 'soniox'
            && (this.currentSource === 'system' || this.currentSource === 'both')
            && this._isSuggestionsMode();
    }
,


    _earlyInit() {
        this._early = new EarlyScheduler({
            fire: (snap, meta) => this._earlyFire(snap, meta),
            cancel: (id) => this._earlyCancel(id),
        });
        this._earlyRequestSeq = 0;
        this._earlyRequestTurn = new Map(); // requestId → {epoch, turnId}
        this._earlyHold = 'none';           // 'none' | 'auto' | 'manual'
        this._earlyTicker = null;
        this._earlyContextSnippets = [];    // Task 7 fills this
        window.__early = this._early;       // debug/tests handle

        document.getElementById('btn-early-hold')?.addEventListener('click', () => {
            this._earlySetHold('manual');
        });
        document.getElementById('btn-early-resume')?.addEventListener('click', () => {
            this._earlySetHold('none');
        });
    }
,


    _earlyStartTicker() {
        if (this._earlyTicker) return;
        this._earlyTicker = setInterval(() => this._earlyTick(), EarlyScheduler.TICK_MS);
    }
,

    _earlyStopTicker() {
        if (this._earlyTicker) {
            clearInterval(this._earlyTicker);
            this._earlyTicker = null;
        }
    }
,


    _earlyTick() {
        if (!this.isRunning || !this._earlySuggestionsEligible()) return;
        const ui = this.transcriptUI;
        const committed = ui.committedTextBySource('system');
        // Snapshot = last committed system line + live provisional tail.
        const lastLine = committed.split('\n').pop() || '';
        const snapshot = (lastLine + ' ' + (ui.provisionalBySource.system.text || '')).trim();
        this._early.observe(snapshot, 'system');
    }
,


    _earlyFire(snapshot, meta) {
        const requestId = ++this._earlyRequestSeq;
        this._earlyRequestTurn.set(requestId, meta);

        const channel = new window.__TAURI__.core.Channel();
        channel.onmessage = (ev) => this._earlyOnEvent(requestId, ev);

        this._setInterviewSuggestionsStatus('Early suggestion…');
        this._earlyRenderDraft('');
        // Show the Hold control only while an early hint is live.
        const holdBtn = document.getElementById('btn-early-hold');
        if (holdBtn && this._earlyHold === 'none') holdBtn.style.display = '';

        // Pre-fetch lexical excerpts — a local sqlite read, far cheaper than
        // the embed+Pinecone path the manual suggestion flow uses.
        invoke('select_context_excerpts', {
            userId: this._getInterviewUserId(),
            query: snapshot,
            maxChars: 3000,
        }).then((excerpts) => {
            if (!this._earlyRequestTurn.has(requestId)) return; // cancelled meanwhile
            const snippets = (excerpts || []).map((e) => `[${e.docType}] ${e.text}`);
            return invoke('suggest_interview_answers_stream', {
                req: {
                    userId: this._getInterviewUserId(),
                    transcriptContext: snapshot,
                    userDraft: null,
                    appMode: 'Interview',
                    contextSnippets: snippets,
                },
                requestId,
                channel,
            });
        }).catch((e) => {
            this._earlyRequestTurn.delete(requestId);
            if (this._early.inFlightId === requestId) this._early.requestDone(requestId);
            console.warn('[Early] invoke failed:', e);
        });

        return requestId;
    }
,


    _earlyCancel(requestId) {
        invoke('cancel_suggestion_stream', { requestId }).catch(() => {});
        this._earlyRequestTurn.delete(requestId);
    }
,


    _earlyOnEvent(requestId, ev) {
        const meta = this._earlyRequestTurn.get(requestId);
        // Drop anything whose epoch/turn no longer matches — stale stream.
        if (!meta || meta.epoch !== this._early.epoch) return;

        switch (ev.kind) {
            case 'delta':
                if (this._earlyHold === 'none') {
                    this._earlyAppendDraft(ev.text || '');
                }
                break;
            case 'done':
                this._earlyRequestTurn.delete(requestId);
                this._early.requestDone(requestId);
                if (this._earlyHold === 'none') {
                    this._earlyPromoteDraft(ev.text || '');
                }
                break;
            case 'insufficient':
                this._earlyRequestTurn.delete(requestId);
                this._early.requestDone(requestId);
                this._setInterviewSuggestionsStatus('Waiting for more of the question…');
                break;
            case 'cancelled':
                this._earlyRequestTurn.delete(requestId);
                break;
            case 'error':
                this._earlyRequestTurn.delete(requestId);
                this._early.requestDone(requestId);
                this._setInterviewSuggestionsStatus('');
                break;
        }
    }
,


    // ─── Draft card (early hint) ───────────────────────────

    _earlyDraftEl() {
        let el = document.getElementById('early-hint-card');
        if (!el) {
            const list = document.getElementById('interview-suggestions-list');
            const panel = document.getElementById('interview-suggestions-panel');
            if (!panel) return null;
            el = document.createElement('div');
            el.id = 'early-hint-card';
            el.className = 'early-hint-card';
            el.innerHTML = `<span class="early-hint-badge">Early</span><span class="early-hint-text"></span>`;
            (list?.parentElement || panel).insertBefore(el, list);
        }
        return el;
    }
,

    _earlyRenderDraft(text) {
        const el = this._earlyDraftEl();
        if (!el) return;
        el.querySelector('.early-hint-text').textContent = text;
        el.style.display = '';
        this._setRightPanelCollapsed?.(false);
        const panel = document.getElementById('interview-suggestions-panel');
        if (panel) panel.style.display = '';
    }
,

    _earlyAppendDraft(delta) {
        const el = this._earlyDraftEl();
        if (!el) return;
        const t = el.querySelector('.early-hint-text');
        t.textContent += delta;
    }
,

    _earlyPromoteDraft(fullText) {
        const el = document.getElementById('early-hint-card');
        const text = (fullText || el?.querySelector('.early-hint-text')?.textContent || '').trim();
        if (!text) return;
        // Promote into the regular suggestion list as a plain-text item.
        const items = text.split('\n').map((l) => l.replace(/^[-•]\s*/, '').trim()).filter(Boolean);
        const normalized = this._normalizeInterviewSuggestionItems(items);
        this._interviewSuggestionsItems = normalized;
        this._renderInterviewSuggestions(normalized);
        if (el) el.remove();
        this._setInterviewSuggestionsStatus('');
    }
,


    // ─── Hold / Resume ─────────────────────────────────────

    _earlySetHold(mode) {
        this._earlyHold = mode;
        const holdBtn = document.getElementById('btn-early-hold');
        const resumeBtn = document.getElementById('btn-early-resume');
        if (holdBtn) holdBtn.style.display = mode === 'none' ? '' : 'none';
        if (resumeBtn) resumeBtn.style.display = mode === 'none' ? 'none' : '';
        if (mode === 'auto') this._setInterviewSuggestionsStatus('Held while you speak');
        else if (mode === 'manual') this._setInterviewSuggestionsStatus('Held');
        else this._setInterviewSuggestionsStatus('');
    }
,

    _onCandidateSpeechFinal() {
        // Candidate started answering — freeze the visible hint.
        if (this._earlyHold === 'none' && this._interviewSuggestionsItems?.length) {
            this._earlySetHold('auto');
        }
    }
,

    _earlyBumpEpoch() {
        this._early?.bumpEpoch();
        this._earlyRequestTurn?.clear();
        this._earlySetHold?.('none');
        document.getElementById('early-hint-card')?.remove();
    }
,
};
