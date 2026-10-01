import { describe, test, expect, vi, beforeEach, afterEach } from 'vitest';
import { SessionMetrics } from '../src/js/metrics.js';

beforeEach(() => vi.useFakeTimers().setSystemTime(1_000_000));
afterEach(() => vi.useRealTimers());

describe('SessionMetrics', () => {
    test('empty ledger returns zeros, never NaN', () => {
        const m = new SessionMetrics();
        const s = m.summary();
        expect(s.ttfhMs).toBe(0);
        expect(s.ttfhAfterEndpointMs).toBe(0);
        expect(s.hintsFired).toBe(0);
        expect(s.hintsCancelled).toBe(0);
        expect(s.hintsUseful).toBe(0);
        expect(Number.isFinite(s.peakReqPerMin)).toBe(true);
    });

    test('TTFH measured from first token, not done', () => {
        const m = new SessionMetrics();
        m.mark('question_start');
        vi.setSystemTime(1_000_000 + 400);
        m.mark('hint_fired', { requestId: 1 });
        vi.setSystemTime(1_000_000 + 900);
        m.mark('hint_first_token', { requestId: 1 });
        vi.setSystemTime(1_000_000 + 3000);
        m.mark('hint_done', { requestId: 1 });
        expect(m.summary().ttfhMs).toBe(900); // first_token (t+900) − question_start (t0)
    });

    test('hint fired before endpoint gives negative TTFH-after-endpoint', () => {
        const m = new SessionMetrics();
        m.mark('question_start');
        vi.setSystemTime(1_000_000 + 500);
        m.mark('hint_fired', { requestId: 1 });
        vi.setSystemTime(1_000_000 + 800);
        m.mark('hint_first_token', { requestId: 1 });
        vi.setSystemTime(1_000_000 + 2000);
        m.mark('endpoint');
        const s = m.summary();
        expect(s.ttfhAfterEndpointMs).toBe(-1200); // 800 - 2000
    });

    test('correction-cancelled hints excluded from useful', () => {
        const m = new SessionMetrics();
        m.mark('question_start');
        m.mark('hint_fired', { requestId: 1 });
        m.mark('hint_done', { requestId: 1 });
        m.mark('hint_cancelled', { requestId: 2, reason: 'correction' });
        m.mark('hint_fired', { requestId: 3 });
        m.mark('hint_done', { requestId: 3 });
        m.mark('mic_speech_start');
        const s = m.summary();
        expect(s.hintsFired).toBe(2);
        expect(s.hintsCancelled).toBe(1);
        expect(s.hintsUseful).toBe(2); // 1 and 3 completed before mic started
    });

    test('peak requests per minute reflects burst', () => {
        const m = new SessionMetrics();
        for (let i = 0; i < 5; i++) {
            m.mark('hint_fired', { requestId: i });
            vi.setSystemTime(1_000_000 + i * 10_000); // 5 in < 1 min
        }
        vi.setSystemTime(1_000_000 + 5 * 60_000);
        m.mark('hint_fired', { requestId: 99 });
        expect(m.summary().peakReqPerMin).toBe(5);
    });
});
