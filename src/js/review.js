// Post-session review UI — docked in the right panel while viewing a saved
// conversation in read-only mode. One LLM call per generation, cached by the
// backend as <name>.review.json. Methods are merged onto App.prototype.

import { invoke } from './ipc.js';

export function formatScore(score) {
    if (score == null) return '—';
    const n = Math.max(0, Math.min(5, score));
    return '●'.repeat(n) + '○'.repeat(5 - n);
}

export const reviewMethods = {

    _reviewInit() {
        if (this._reviewInited) return;
        this._reviewInited = true;
        document.getElementById('btn-review-regenerate')
            ?.addEventListener('click', () => {
                if (this.activeConversationFilename) {
                    this._reviewGenerate(this.activeConversationFilename, true);
                }
            });
        document.getElementById('btn-review-copy')
            ?.addEventListener('click', () => this._reviewCopy());
        document.getElementById('btn-review-close')
            ?.addEventListener('click', () => this._reviewHide());
    }
,

    /// Called after a conversation's segments load in read-only mode. Shows the
    /// cached review when one exists; otherwise just makes the banner button
    /// available (generation always waits for the explicit click).
    async _reviewLoadCached(filename) {
        try {
            const cached = await invoke('read_session_review', { filename });
            if (cached) {
                this._review = cached;
                this._reviewFilename = filename;
                this._reviewRender(cached);
                this._reviewShow();
            } else {
                this._review = null;
                this._reviewHide();
            }
        } catch (err) {
            console.warn('[Review] cache read failed:', err);
            this._reviewHide();
        }
    }
,

    async _reviewGenerate(filename, force) {
        if (!filename) return;
        if (this._review && this._reviewFilename === filename && !force) {
            this._reviewShow();
            return;
        }
        const kb = this._reviewTranscriptKb ? ` ~${this._reviewTranscriptKb} KB of transcript` : '';
        const verb = force ? 'Regenerate the review' : 'Review this interview';
        if (!confirm(`${verb} with the LLM? One request,${kb}.`)) return;

        const status = document.getElementById('review-status');
        if (status) status.textContent = 'Reviewing…';
        try {
            const review = await invoke('review_session', { filename, force });
            this._review = review;
            this._reviewFilename = filename;
            this._reviewRender(review);
            this._reviewShow();
            this._loadConversationList?.(); // refresh ★ badge
        } catch (err) {
            if (status) status.textContent = '';
            this._showToast(`Review failed: ${err}`, 'error');
        }
    }
,

    _reviewShow() {
        const panel = document.getElementById('review-panel');
        const right = document.getElementById('right-panel');
        const contentArea = document.getElementById('content-area');
        const suggestions = document.getElementById('interview-suggestions-panel');
        if (!panel || !right) return;
        panel.hidden = false;
        right.style.display = '';
        contentArea?.classList.add('split-suggestions');
        // Review takes over the whole right panel — clear the suggestions
        // collapsed state so the panel isn't hidden.
        contentArea?.classList.remove('right-panel-collapsed');
        this._rightPanelCollapsed = false;
        const btnOpen = document.getElementById('btn-open-suggestions');
        if (btnOpen) btnOpen.style.display = 'none';
        if (suggestions) suggestions.style.display = 'none';
    }
,

    _reviewHide() {
        const panel = document.getElementById('review-panel');
        if (!panel || panel.hidden) return;
        panel.hidden = true;
        const status = document.getElementById('review-status');
        if (status) status.textContent = '';
        // Hand the right panel back to the docked suggestions panel, or hide
        // the panel entirely when nothing else needs it.
        const suggestions = document.getElementById('interview-suggestions-panel');
        const right = document.getElementById('right-panel');
        const contentArea = document.getElementById('content-area');
        if (this._suggestionsDock?.docked && suggestions) {
            suggestions.style.display = '';
            // Restore the edge toggle for the docked suggestions panel.
            this._setRightPanelCollapsed?.(this._rightPanelCollapsed);
        } else {
            if (right) right.style.display = 'none';
            contentArea?.classList.remove('split-suggestions');
            contentArea?.classList.remove('right-panel-collapsed');
        }
    }
,

    _reviewRender(review) {
        const status = document.getElementById('review-status');
        const overall = document.getElementById('review-overall');
        const list = document.getElementById('review-questions');
        const practiceWrap = document.getElementById('review-practice-wrap');
        const practice = document.getElementById('review-practice');
        if (!list) return;

        if (status) {
            status.textContent = review.roles_known
                ? ''
                : 'Speaker roles were inferred — this transcript predates source tagging.';
        }
        if (overall) overall.textContent = review.overall || '';

        list.innerHTML = '';
        for (const q of review.questions || []) {
            const li = document.createElement('li');
            li.className = 'review-card';

            const qEl = document.createElement('div');
            qEl.className = 'review-q';
            qEl.textContent = q.question;
            li.appendChild(qEl);

            if (q.answer_summary) {
                const a = document.createElement('div');
                a.className = 'review-a';
                a.textContent = `You said: ${q.answer_summary}`;
                li.appendChild(a);
            }

            const meta = document.createElement('div');
            meta.className = 'review-meta';
            const score = document.createElement('span');
            score.className = 'review-score';
            score.textContent = formatScore(q.score);
            meta.appendChild(score);
            const fb = document.createElement('span');
            fb.className = 'review-feedback';
            fb.textContent = q.feedback;
            meta.appendChild(fb);
            li.appendChild(meta);

            if (q.stronger_answer) {
                const s = document.createElement('div');
                s.className = 'review-stronger';
                s.textContent = `Try: ${q.stronger_answer}`;
                li.appendChild(s);
            }
            list.appendChild(li);
        }

        const items = review.practice || [];
        if (practiceWrap && practice) {
            practiceWrap.hidden = items.length === 0;
            practice.innerHTML = '';
            for (const p of items) {
                const li = document.createElement('li');
                li.textContent = p;
                practice.appendChild(li);
            }
        }
    }
,

    async _reviewCopy() {
        const r = this._review;
        if (!r) return;
        const lines = [`Overall: ${r.overall}`, ''];
        for (const q of r.questions || []) {
            lines.push(`Q: ${q.question}`);
            if (q.answer_summary) lines.push(`A: ${q.answer_summary}`);
            lines.push(`Score: ${formatScore(q.score)}  ${q.feedback}`);
            if (q.stronger_answer) lines.push(`Better: ${q.stronger_answer}`);
            lines.push('');
        }
        if (r.practice?.length) {
            lines.push('Practice:');
            r.practice.forEach((p) => lines.push(`- ${p}`));
        }
        try {
            await navigator.clipboard.writeText(lines.join('\n'));
            this._showToast('Review copied', 'success');
        } catch {
            this._showToast('Copy failed', 'error');
        }
    }
,
};
