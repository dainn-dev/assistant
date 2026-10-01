// Per-session early-suggestion metrics — in-memory event ledger, summarized
// into the transcript sidecar on save. No network, no identifiers.

const WINDOW_MS = 60_000;

export class SessionMetrics {
    constructor(now = () => Date.now()) {
        this._now = now;
        this._events = [];        // { t, event, requestId?, reason? }
    }

    mark(event, payload = {}) {
        this._events.push({ t: this._now(), event, ...payload });
    }

    _first(event, requestId) {
        return this._events.find(
            (e) => e.event === event && (requestId === undefined || e.requestId === requestId)
        );
    }

    summary() {
        const questionStart = this._first('question_start');
        const endpoint = this._first('endpoint');
        const fired = this._events.filter((e) => e.event === 'hint_fired');
        const cancelled = this._events.filter((e) => e.event === 'hint_cancelled');
        const micStarts = this._events.filter((e) => e.event === 'mic_speech_start');

        // First token across all requests — the moment a useful hint appeared.
        const firstToken = this._first('hint_first_token');

        // Useful = done, not cancelled, and completed before the candidate
        // started speaking. Mic speech is matched per-turn when turnIds are
        // present; unturned events match any turn.
        const cancelledIds = new Set(cancelled.map((e) => e.requestId));
        const useful = fired.filter((f) => {
            if (cancelledIds.has(f.requestId)) return false;
            const done = this._first('hint_done', f.requestId);
            if (!done) return false;
            const micStart = micStarts.find(
                (m) => m.turnId === undefined || f.turnId === undefined || m.turnId === f.turnId
            );
            return !micStart || done.t <= micStart.t;
        });

        // Peak hints/min over sliding windows anchored at each fire.
        let peak = 0;
        for (const f of fired) {
            const inWindow = fired.filter((o) => o.t - f.t < WINDOW_MS && o.t >= f.t).length;
            if (inWindow > peak) peak = inWindow;
        }

        return {
            ttfhMs: questionStart && firstToken ? firstToken.t - questionStart.t : 0,
            ttfhAfterEndpointMs: endpoint && firstToken ? firstToken.t - endpoint.t : 0,
            hintsFired: fired.length,
            hintsCancelled: cancelled.length,
            hintsUseful: useful.length,
            peakReqPerMin: peak,
        };
    }
}
