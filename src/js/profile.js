// Candidate Profile — persistent summary, STAR stories and strengths stored in
// SQLite (profile_items) and merged into the excerpt selection that grounds
// Interview suggestions. Methods are merged onto App.prototype.
// Extracted pattern: same mixin style as session.js / interview-panel.js.

import { invoke } from './ipc.js';

const SAVE_DEBOUNCE_MS = 600;
const PROFILE_KINDS = { story: 'STORY', strength: 'STRENGTH' };

/**
 * Merge an LLM draft into existing items. A draft summary replaces the existing
 * one; draft stories/strengths whose title already exists are dropped. Draft
 * items are flagged `_draft` so the UI can gate them behind Accept/Discard.
 */
export function mergeDraft(existing, draft) {
    const titles = new Set(
        existing
            .filter((i) => i.kind !== 'summary')
            .map((i) => (i.title || '').trim().toLowerCase())
    );
    let draftSummary = null;
    const kept = [];
    for (const d of draft || []) {
        const item = { ...d, _draft: true };
        if (d.kind === 'summary') {
            draftSummary = item;
            continue;
        }
        if (titles.has((d.title || '').trim().toLowerCase())) continue;
        kept.push(item);
    }
    const summary =
        draftSummary || existing.find((i) => i.kind === 'summary') || null;
    const others = existing.filter((i) => i.kind !== 'summary');
    return [summary, ...kept, ...others].filter(Boolean);
}

export const profileMethods = {

    _profileInit() {
        if (this._profileInited) return;
        this._profileInited = true;
        this._profileItems = [];
        this._profilePreDraftItems = null;
        this._profileSaveTimers = new Map();
        this._profileSeq = 0;

        document.getElementById('btn-profile-add-story')
            ?.addEventListener('click', () => this._profileAddItem('story'));
        document.getElementById('btn-profile-add-strength')
            ?.addEventListener('click', () => this._profileAddItem('strength'));
        document.getElementById('btn-profile-draft')
            ?.addEventListener('click', () => this._profileDraft());
        document.getElementById('btn-profile-accept')
            ?.addEventListener('click', () => this._profileAcceptDraft());
        document.getElementById('btn-profile-discard')
            ?.addEventListener('click', () => this._profileDiscardDraft());
        document.getElementById('profile-summary')?.addEventListener('input', (e) => {
            this._profileQueueSave({ id: 'summary', kind: 'summary', title: '', content: e.target.value });
        });

        this._profileLoad();
    }
,

    async _profileLoad() {
        try {
            const items = await invoke('list_profile', { userId: this._getInterviewUserId() });
            this._profileItems = (items || []).map((i) => ({ ...i, _draft: false }));
            this._profilePreDraftItems = null;
            this._profileRender();
        } catch (err) {
            this._profileStatus(`Could not load profile: ${err}`);
        }
    }
,

    _profileRender() {
        const summaryEl = document.getElementById('profile-summary');
        const listEl = document.getElementById('profile-items');
        const summary = this._profileItems.find((i) => i.kind === 'summary');
        if (summaryEl && document.activeElement !== summaryEl) {
            summaryEl.value = summary ? summary.content : '';
        }
        if (!listEl) return;
        listEl.innerHTML = '';
        const nonSummary = this._profileItems.filter((i) => i.kind !== 'summary');

        if (!nonSummary.length) {
            const li = document.createElement('li');
            li.className = 'profile-empty';
            li.textContent = 'No stories yet — write one, or draft from your CV.';
            listEl.appendChild(li);
        }

        for (const item of nonSummary) {
            const li = document.createElement('li');
            li.className = 'profile-item' + (item._draft ? ' draft' : '');
            li.dataset.id = item.id;

            const head = document.createElement('div');
            head.className = 'profile-item-head';

            const chip = document.createElement('span');
            chip.className = 'profile-kind';
            chip.textContent = PROFILE_KINDS[item.kind] || item.kind.toUpperCase();
            head.appendChild(chip);

            const title = document.createElement('input');
            title.className = 'profile-title';
            title.value = item.title || '';
            title.placeholder = item.kind === 'story' ? 'Story title' : 'Skill';
            title.addEventListener('input', (e) =>
                this._profileQueueSave({ ...item, title: e.target.value })
            );
            head.appendChild(title);

            const del = document.createElement('button');
            del.className = 'icon-btn small profile-delete';
            del.textContent = '×';
            del.title = 'Delete';
            del.addEventListener('click', () => this._profileDeleteItem(item));
            head.appendChild(del);

            const content = document.createElement('textarea');
            content.className = 'profile-content';
            content.rows = item.kind === 'story' ? 4 : 2;
            content.value = item.content || '';
            content.placeholder =
                item.kind === 'story'
                    ? 'Situation: … Task: … Action: … Result: …'
                    : 'Evidence / one line';
            content.addEventListener('input', (e) =>
                this._profileQueueSave({ ...item, content: e.target.value })
            );

            li.appendChild(head);
            li.appendChild(content);
            listEl.appendChild(li);
        }

        this._profileUpdateDraftBar();
    }
,

    _profileUpdateDraftBar() {
        const bar = document.getElementById('profile-draft-bar');
        const count = document.getElementById('profile-draft-count');
        const drafts = this._profileItems.filter((i) => i._draft);
        if (bar) bar.hidden = drafts.length === 0;
        if (count) count.textContent = `${drafts.length} drafted item${drafts.length === 1 ? '' : 's'} — review, then`;
    }
,

    _profileAddItem(kind) {
        const item = {
            id: `p-${Date.now()}-${(this._profileSeq = (this._profileSeq || 0) + 1)}`,
            kind,
            title: '',
            content: '',
            _draft: false,
        };
        this._profileItems.push(item);
        this._profileRender();
    }
,

    async _profileDeleteItem(item) {
        this._profileItems = this._profileItems.filter((i) => i.id !== item.id);
        this._profileRender();
        if (item._draft) return; // never persisted — nothing to delete
        try {
            await invoke('delete_profile_item', {
                userId: this._getInterviewUserId(),
                id: item.id,
            });
        } catch (err) {
            this._profileStatus(`Delete failed: ${err}`);
        }
    }
,

    _profileQueueSave(item) {
        const existing = this._profileItems.find((i) => i.id === item.id);
        if (existing) Object.assign(existing, item);
        else this._profileItems.push({ ...item });

        if (!this._profileSaveTimers) this._profileSaveTimers = new Map();
        clearTimeout(this._profileSaveTimers.get(item.id));
        this._profileSaveTimers.set(
            item.id,
            setTimeout(() => {
                this._profileSaveTimers.delete(item.id);
                const it = this._profileItems.find((i) => i.id === item.id);
                if (!it) return;
                invoke('save_profile_item', {
                    item: {
                        id: it.id,
                        user_id: this._getInterviewUserId(),
                        kind: it.kind,
                        title: it.title || '',
                        content: it.content || '',
                        updated_at: new Date().toISOString(),
                    },
                }).catch((err) => this._profileStatus(`Save failed: ${err}`));
            }, SAVE_DEBOUNCE_MS)
        );
    }
,

    async _profileDraft() {
        const btn = document.getElementById('btn-profile-draft');
        if (btn) {
            btn.disabled = true;
            btn.textContent = 'Drafting…';
        }
        try {
            const draft = await invoke('draft_profile_from_documents', {
                userId: this._getInterviewUserId(),
            });
            this._profilePreDraftItems = this._profileItems.map((i) => ({ ...i }));
            this._profileItems = mergeDraft(this._profileItems, draft || []);
            this._profileRender();
        } catch (err) {
            this._profileStatus(`Draft failed: ${err}`);
        } finally {
            if (btn) {
                btn.disabled = false;
                btn.textContent = 'Draft from CV';
            }
        }
    }
,

    _profileAcceptDraft() {
        const drafts = this._profileItems.filter((i) => i._draft);
        for (const d of drafts) {
            d._draft = false;
            this._profileQueueSave(d);
        }
        this._profilePreDraftItems = null;
        this._profileRender();
        this._profileStatus(`${drafts.length} item${drafts.length === 1 ? '' : 's'} saved to profile`);
    }
,

    _profileDiscardDraft() {
        if (this._profilePreDraftItems) {
            this._profileItems = this._profilePreDraftItems;
        } else {
            this._profileItems = this._profileItems.filter((i) => !i._draft);
        }
        this._profilePreDraftItems = null;
        this._profileRender();
        this._profileStatus('Draft discarded');
    }
,

    _profileStatus(text) {
        const el = document.getElementById('profile-status');
        if (el) el.textContent = text;
    }
,
};
