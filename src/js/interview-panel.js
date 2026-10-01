// Interview mode: suggestions panel, uploads/ingest, chat, streaming render
// Extracted from app.js — methods are merged onto App.prototype via Object.assign.

import { settingsManager } from './settings.js';

import { invoke, listen } from './ipc.js';


export const interviewPanelMethods = {

    _isSuggestionsMode() {
        return this.currentTemplate === 'Interview';
    }
,


    _suggestionsPanelTitle() {
        return 'Suggested answers';
    }
,


    _suggestionsEmptyText() {
        return 'Waiting for interviewer question…';
    }
,


    _updateSuggestionsPanelChrome() {
        const title = document.getElementById('interview-suggestions-title');
        if (title) title.textContent = this._suggestionsPanelTitle();
        this._updateSuggestionsEmptyState();
    }
,


    _updateSuggestionsEmptyState() {
        const empty = document.getElementById('interview-suggestions-empty');
        if (!empty) return;
        const show = this._isSuggestionsMode()
            && !this._interviewSuggestionsItems.length
            && !this._interviewSuggestionsClosed;

        // Actionable hint when suggestions can't run — LLM not configured.
        const s = settingsManager.get();
        const llmReady = !!(s.llm_url && s.llm_model && s.llm_api_key);
        if (show && !llmReady) {
            empty.textContent = 'Add an LLM endpoint in Settings → AI to enable suggestions';
            empty.classList.add('actionable');
            if (!empty._bound) {
                empty._bound = true;
                empty.addEventListener('click', () => {
                    this._showView('settings');
                    document.querySelector('.settings-tab[data-tab="tab-interview"]')?.click();
                });
            }
        } else {
            empty.textContent = this._suggestionsEmptyText();
            empty.classList.remove('actionable');
        }
        empty.hidden = !show;
    }
,


    _clearSuggestionsPanel() {
        this._interviewSuggestGen += 1;
        clearTimeout(this._interviewSuggestTimer);
        this._cancelInterviewSuggestionsStreaming();
        this._interviewSuggestionsItems = [];
        this._lastInterviewSuggestArgs = { transcriptContext: null, userDraft: null };
        this._brainstormPending = false;
        this._interviewSuggestionsClosed = false;
        const list = document.getElementById('interview-suggestions-list');
        if (list) list.innerHTML = '';
        this._setInterviewSuggestionsStatus('');
        document.getElementById('transcript-content')?.querySelectorAll('.seg-brainstorm-btn').forEach((el) => el.remove());
        this._updateSuggestionsEmptyState();
    }
,


    _setTemplateMode(mode) {
        const prevMode = this.currentTemplate;
        this.currentTemplate = mode || null;
        if (prevMode !== this.currentTemplate) this._earlyBumpEpoch?.();
        if (
            prevMode !== this.currentTemplate
            && prevMode
            && this.currentTemplate
            && prevMode === 'Interview'
            && this.currentTemplate === 'Interview'
        ) {
            this._clearSuggestionsPanel();
        }
        if (this._isSuggestionsMode()) {
            this._interviewSuggestionsClosed = false;
        }

        // Mobile: mark body classes for mode-specific UI
        document.body.classList.toggle('interview-active', this.currentTemplate === 'Interview');
        document.body.classList.toggle('suggestions-active', this._isSuggestionsMode());

        const uploads = document.getElementById('interview-uploads');
        if (uploads) uploads.style.display = this.currentTemplate === 'Interview' ? '' : 'none';
        const sugPanel = document.getElementById('interview-suggestions-panel');
        if (!this._isSuggestionsMode()) {
            if (sugPanel) sugPanel.style.display = 'none';
            this._undockInterviewSuggestions();
            this._rightPanelCollapsed = false;
            this._interviewSuggestGen += 1;
            if (this.isMobile) this._setMobileSheetOpen(false);
        } else {
            if (sugPanel) sugPanel.style.display = '';
            this._updateSuggestionsPanelChrome();
            if (this.isMobile) {
                this._undockInterviewSuggestions();
                this._setMobileSheetOpen(false);
            } else {
                this._dockInterviewSuggestionsRight();
                this._setRightPanelCollapsed(true);
            }
            if (this.currentTemplate === 'Interview') {
                this._scheduleInterviewIngest();
            }
        }
        this._updateChatInputState();
        this._profileSyncTabVisibility?.();
    }
,


    _dockInterviewSuggestionsRight() {
        if (this.isMobile) return;
        const panel = document.getElementById('interview-suggestions-panel');
        const right = document.getElementById('right-panel');
        const contentArea = document.getElementById('content-area');
        if (!panel || !right || !contentArea) return;

        // Record original DOM position once, so we can restore later.
        if (!this._suggestionsDock.originalParent) {
            this._suggestionsDock.originalParent = panel.parentElement;
            this._suggestionsDock.originalNextSibling = panel.nextSibling;
        }

        if (panel.parentElement !== right) {
            right.appendChild(panel);
        }

        right.style.display = '';
        contentArea.classList.add('split-suggestions');
        panel.classList.add('docked-right');
        this._suggestionsDock.docked = true;
    }
,


    _setRightPanelCollapsed(collapsed) {
        const contentArea = document.getElementById('content-area');
        const btnOpen = document.getElementById('btn-open-suggestions');
        const btnClose = document.getElementById('btn-close-suggestions');
        if (!contentArea || !btnOpen || !btnClose) return;
        this._rightPanelCollapsed = !!collapsed;
        contentArea.classList.toggle('right-panel-collapsed', this._rightPanelCollapsed);
        btnOpen.style.display = this._rightPanelCollapsed ? '' : 'none';
        btnOpen.setAttribute('aria-expanded', String(!this._rightPanelCollapsed));
        btnClose.style.display = this._rightPanelCollapsed ? 'none' : '';
    }
,


    _setMobileSheetOpen(open) {
        if (!this.isMobile) return;
        document.body.classList.toggle('sheet-open', !!open);
        // Ensure sidebar state doesn't conflict with sheet UX.
        if (open) {
            this.sidebarOpen = false;
            document.body.classList.remove('sidebar-open');
            document.getElementById('sidebar')?.classList.add('hidden');
        }
    }
,


    _undockInterviewSuggestions() {
        const panel = document.getElementById('interview-suggestions-panel');
        const right = document.getElementById('right-panel');
        const contentArea = document.getElementById('content-area');
        if (!panel || !right || !contentArea) return;

        panel.classList.remove('docked-right');

        const { originalParent, originalNextSibling } = this._suggestionsDock;
        if (originalParent) {
            if (originalNextSibling && originalNextSibling.parentNode === originalParent) {
                originalParent.insertBefore(panel, originalNextSibling);
            } else {
                originalParent.appendChild(panel);
            }
        }

        right.style.display = 'none';
        contentArea.classList.remove('split-suggestions');
        contentArea.classList.remove('right-panel-collapsed');
        this._rightPanelCollapsed = false;
        const btnOpen = document.getElementById('btn-open-suggestions');
        if (btnOpen) btnOpen.style.display = 'none';
        this._suggestionsDock.docked = false;
    }
,


    _isAllowedInterviewFile(filename) {
        const name = String(filename || '').toLowerCase();
        return name.endsWith('.pdf') || name.endsWith('.docx');
    }
,


    _updateInterviewUploadPills() {
        const pillCv = document.getElementById('pill-cv');
        const pillCvName = document.getElementById('pill-cv-name');
        const pillJd = document.getElementById('pill-jd');
        const pillJdName = document.getElementById('pill-jd-name');

        if (pillCv && pillCvName) {
            if (this._interviewCvFile) {
                pillCvName.textContent = this._interviewCvFile.name || 'CV';
                pillCv.style.display = '';
            } else {
                pillCvName.textContent = '';
                pillCv.style.display = 'none';
            }
        }

        if (pillJd && pillJdName) {
            if (this._interviewJdFile) {
                pillJdName.textContent = this._interviewJdFile.name || 'JD';
                pillJd.style.display = '';
            } else {
                pillJdName.textContent = '';
                pillJd.style.display = 'none';
            }
        }
    }
,


    _initInterviewUploads() {
        const uploads = document.getElementById('interview-uploads');
        const btnUpload = document.getElementById('btn-upload-interview-files');
        const inputFiles = document.getElementById('file-upload-interview');
        const clearCv = document.getElementById('pill-cv-clear');
        const clearJd = document.getElementById('pill-jd-clear');

        if (!uploads || !btnUpload || !inputFiles) return;

        // Default hidden until Interview selected
        uploads.style.display = 'none';

        btnUpload.addEventListener('click', async () => {
            // Prefer the native file dialog: selected paths let the backend
            // read files directly instead of shuttling bytes over IPC.
            const dialog = window.__TAURI__?.dialog;
            if (!dialog?.open) {
                inputFiles.click(); // fallback: hidden <input type=file>
                return;
            }
            try {
                const selected = await dialog.open({
                    multiple: true,
                    filters: [{ name: 'Documents', extensions: ['pdf', 'docx'] }],
                });
                if (!selected) return;
                const paths = Array.isArray(selected) ? selected : [selected];
                // Selection order: first = CV, second = JD (same convention as input fallback)
                const files = paths.slice(0, 2).map((p) => ({ name: String(p).split(/[\\/]/).pop(), path: p }));
                this._interviewCvFile = files[0] || null;
                this._interviewJdFile = files[1] || null;
                this._updateInterviewUploadPills();
                this._scheduleInterviewIngest();
            } catch (e) {
                console.warn('[Interview] native picker failed, falling back to file input', e);
                inputFiles.click();
            }
        });

        inputFiles.addEventListener('change', () => {
            const files = Array.from(inputFiles.files || []);
            if (!files.length) return;

            const allowed = files.filter(f => this._isAllowedInterviewFile(f.name));
            if (!allowed.length) {
                inputFiles.value = '';
                this._showToast('Please choose .pdf or .docx files', 'error');
                return;
            }

            // Use selection order: first = CV, second = JD (if present)
            this._interviewCvFile = allowed[0] || null;
            this._interviewJdFile = allowed[1] || null;
            this._updateInterviewUploadPills();
            this._scheduleInterviewIngest();
        });

        clearCv?.addEventListener('click', () => {
            this._interviewCvFile = null;
            inputFiles.value = '';
            this._updateInterviewUploadPills();
        });

        clearJd?.addEventListener('click', () => {
            this._interviewJdFile = null;
            inputFiles.value = '';
            this._updateInterviewUploadPills();
        });

        this._updateInterviewUploadPills();
    }
,


    _sendChatMessage() {
        const input = document.getElementById('chat-input');
        if (!input) return;
        const text = (input.value || '').trim();
        if (!text) return;

        input.value = '';
        this.transcriptUI?.addChatMessage?.(text, 'ME');
        this._consumePickedSuggestion(text);
        if (this.currentTemplate === 'Interview') {
            (async () => {
                try {
                    await invoke('save_interview_message', {
                        req: { userId: this._getInterviewUserId(), role: 'user', content: text },
                    });
                } catch (e) {
                    console.warn('[Interview] save user message', e);
                }
                this._lastInterviewSuggestArgs = { transcriptContext: null, userDraft: text };
                this._interviewSuggestionsClosed = false;
                this._setRightPanelCollapsed(false);
                this._markInterviewSuggestStart('draft');
                this._scheduleSuggestions({ transcriptContext: null, userDraft: text });
            })();
        }
    }
,


    _consumePickedSuggestion(sentText) {
        const picked = this._pickedSuggestion;
        this._pickedSuggestion = null;
        if (!picked || picked.id == null) return;
        if (!sentText || !String(sentText).includes(picked.text)) return;

        const next = this._interviewSuggestionsItems.filter((it) => it.id !== picked.id);
        if (next.length === this._interviewSuggestionsItems.length) return;

        this._renderInterviewSuggestions(next);
    }
,


    _getInterviewUserId() {
        const KEY = 'myjavis_interview_user_id';
        let id = localStorage.getItem(KEY);
        if (!id) {
            id = typeof crypto !== 'undefined' && crypto.randomUUID ? crypto.randomUUID() : `u_${Date.now()}`;
            localStorage.setItem(KEY, id);
        }
        return id;
    }
,


    _scheduleInterviewIngest() {
        if (this.currentTemplate !== 'Interview') return;
        if (!this._interviewCvFile && !this._interviewJdFile) return;
        clearTimeout(this._ingestInterviewDebounce);
        this._ingestInterviewDebounce = setTimeout(() => void this._ingestInterviewFilesNow(), 500);
    }
,


    async _ingestInterviewFilesNow() {
        if (!this._interviewCvFile && !this._interviewJdFile) return;

        const progressEl = document.getElementById('ingest-progress');
        const fillEl = document.getElementById('ingest-progress-fill');
        const labelEl = document.getElementById('ingest-progress-label');

        const stageLabel = { extracting: 'Extracting…', embedding: 'Embedding…', upserting: 'Saving to index…', done: 'Done' };
        const showProgress = (pct, label) => {
            if (progressEl) progressEl.style.display = 'flex';
            if (fillEl) fillEl.style.width = `${pct}%`;
            if (labelEl) labelEl.textContent = label;
        };
        const hideProgress = () => {
            setTimeout(() => { if (progressEl) progressEl.style.display = 'none'; }, 1200);
        };

        showProgress(0, 'Preparing…');
        const unlisten = await listen('ingest:progress', (e) => {
            const { stage, current, total, docType } = e.payload;
            const prefix = docType === 'cv' ? 'CV' : 'JD';
            let pct = 5;
            if (stage === 'embedding') pct = total > 0 ? 10 + Math.round((current / total) * 70) : 10;
            else if (stage === 'upserting') pct = 85;
            else if (stage === 'done') pct = 100;
            showProgress(pct, `${prefix}: ${stageLabel[stage] || stage}`);
        });

        try {
            const userId = this._getInterviewUserId();
            const req = { userId };
            // Files picked via the native dialog carry a real path — the backend
            // reads them directly. Files from the <input> fallback send bytes.
            const buildPart = async (file) => {
                if (file.path) return { filename: file.name, path: file.path };
                const buf = await file.arrayBuffer();
                return { filename: file.name, bytes: Array.from(new Uint8Array(buf)) };
            };
            if (this._interviewCvFile) req.cv = await buildPart(this._interviewCvFile);
            if (this._interviewJdFile) req.jd = await buildPart(this._interviewJdFile);
            const res = await invoke('ingest_interview_files', { req });
            showProgress(100, 'Indexed');
            this._showToast(res.message || 'Documents indexed', 'success');
        } catch (e) {
            if (progressEl) progressEl.style.display = 'none';
            this._showToast(`Ingest failed: ${e}`, 'error');
        } finally {
            unlisten();
            hideProgress();
        }
    }
,


    _onInterviewSpeakerFinal(text, source = 'system') {
        if (!this._isSuggestionsMode()) return;
        // The candidate's own mic speech is context for the scheduler, never a
        // trigger for a new suggestion round.
        if (source === 'mic') {
            this._onCandidateSpeechFinal?.(text);
            return;
        }
        const t = String(text || '').trim();
        if (!t) return;
        this._lastInterviewSuggestArgs = { transcriptContext: t, userDraft: null };
        this._brainstormPending = true;
        this._injectBrainstormButton();
        if (this.currentTemplate === 'Interview') {
            (async () => {
                try {
                    await invoke('save_interview_message', {
                        req: { userId: this._getInterviewUserId(), role: 'speaker', content: t },
                    });
                } catch (e) {
                    console.warn('[Interview] save speaker line', e);
                }
            })();
        }
    }
,


    _injectBrainstormButton() {
        if (!this._brainstormPending) return;
        const content = document.getElementById('transcript-content');
        if (!content) return;
        // Remove any existing brainstorm button first
        content.querySelectorAll('.seg-brainstorm-btn').forEach(el => el.remove());
        // Support both view modes: subtitle-pair (subtitle view) and seg-block (single/dual view)
        const pairs = content.querySelectorAll('.subtitle-pair:not(.pending), .seg-block');
        const lastBlock = pairs[pairs.length - 1];
        if (!lastBlock) return;
        const btn = document.createElement('button');
        btn.type = 'button';
        btn.className = 'seg-brainstorm-btn';
        btn.title = 'Generate answer suggestions';
        btn.innerHTML = `<svg width="26" height="26" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M12 2a7 7 0 0 1 7 7c0 2.5-1.3 4.7-3.3 6l-.7.5V17a2 2 0 0 1-2 2h-2a2 2 0 0 1-2-2v-1.5l-.7-.5A7 7 0 0 1 5 9a7 7 0 0 1 7-7z"/>
            <line x1="9" y1="21" x2="15" y2="21"/>
        </svg>`;
        btn.addEventListener('click', () => {
            this._brainstormPending = false;
            btn.remove();
            this._interviewSuggestionsClosed = false;
            this._setRightPanelCollapsed(false);
            const { transcriptContext, userDraft } = this._lastInterviewSuggestArgs || {};
            this._markInterviewSuggestStart(transcriptContext ? 'speaker' : 'draft');
            this._scheduleSuggestions({ transcriptContext, userDraft });
        });
        lastBlock.appendChild(btn);
    }
,


    _scheduleSuggestions({ transcriptContext, userDraft }) {
        if (!this._isSuggestionsMode()) return;
        // Manual mode: _markInterviewSuggestStart is called by the trigger button
        this._lastInterviewSuggestArgs = {
            transcriptContext: transcriptContext || null,
            userDraft: userDraft || null,
        };
        clearTimeout(this._interviewSuggestTimer);
        const gen = ++this._interviewSuggestGen;
        this._interviewSuggestTimer = setTimeout(() => {
            void this._runInterviewSuggestions(gen, { transcriptContext, userDraft });
        }, 200);
    }
,


    _setInterviewSuggestionsStatus(text) {
        const el = document.getElementById('interview-suggestions-status');
        if (!el) return;
        if (!text) {
            el.style.display = 'none';
            el.textContent = '';
            return;
        }
        el.style.display = '';
        el.textContent = text;
    }
,


    _markInterviewSuggestStart(origin) {
        this._cancelInterviewSuggestionsStreaming();
        if (this._interviewSuggestPerf.hideTimer) {
            clearTimeout(this._interviewSuggestPerf.hideTimer);
            this._interviewSuggestPerf.hideTimer = null;
        }
        this._interviewSuggestPerf.origin = origin || null;
        this._interviewSuggestPerf.t0 = performance.now();
        if (this._interviewSuggestPerf.timer) clearInterval(this._interviewSuggestPerf.timer);
        this._interviewSuggestPerf.timer = setInterval(() => {
            const ms = performance.now() - this._interviewSuggestPerf.t0;
            const s = (ms / 1000).toFixed(ms < 10_000 ? 1 : 0);
            this._setInterviewSuggestionsStatus(`Generating… ${s}s`);
        }, 100);
        this._setInterviewSuggestionsStatus('Generating… 0.0s');
    }
,


    _markInterviewSuggestDone(ok) {
        const t0 = this._interviewSuggestPerf.t0 || performance.now();
        const ms = performance.now() - t0;
        const s = (ms / 1000).toFixed(ms < 10_000 ? 1 : 0);
        if (this._interviewSuggestPerf.timer) {
            clearInterval(this._interviewSuggestPerf.timer);
            this._interviewSuggestPerf.timer = null;
        }
        this._setInterviewSuggestionsStatus(ok ? `Done · ${s}s` : `Failed · ${s}s`);
        this._interviewSuggestPerf.hideTimer = setTimeout(() => {
            this._setInterviewSuggestionsStatus('');
            this._interviewSuggestPerf.hideTimer = null;
        }, 2500);
    }
,


    _cancelInterviewSuggestionsStreaming() {
        const timers = this._interviewSuggestionsStream?.timers || [];
        timers.forEach((t) => {
            clearTimeout(t);
            clearInterval(t);
        });
        this._interviewSuggestionsStream.timers = [];
    }
,


    _renderInterviewSuggestionsStream(items) {
        // Renders list immediately (structure/buttons), then reveals text progressively.
        this._cancelInterviewSuggestionsStreaming();
        const t0 = this._interviewSuggestPerf.t0;
        const ms = t0 ? performance.now() - t0 : 0;
        const renderTime = ms > 0 ? (ms / 1000).toFixed(ms < 10000 ? 1 : 0) + 's' : '';

        const panel = document.getElementById('interview-suggestions-panel');
        const list = document.getElementById('interview-suggestions-list');
        if (!panel || !list) return;

        const normalized = this._normalizeInterviewSuggestionItems(items);
        this._interviewSuggestionsItems = normalized.slice();

        if (this._interviewSuggestionsClosed) {
            panel.style.display = '';
            this._setRightPanelCollapsed(true);
            this._updateSuggestionsEmptyState();
            return;
        }
        if (!this._isSuggestionsMode()) {
            panel.style.display = 'none';
            list.innerHTML = '';
            this._undockInterviewSuggestions();
            return;
        }
        if (!normalized.length) {
            panel.style.display = '';
            list.innerHTML = '';
            this._dockInterviewSuggestionsRight();
            this._setRightPanelCollapsed(true);
            this._updateSuggestionsEmptyState();
            return;
        }

        this._updateSuggestionsEmptyState();

        this._setRightPanelCollapsed(false);
        panel.style.display = '';
        list.innerHTML = '';

        const suggestionType = settingsManager.get().suggestion_type || 'translation';

        const mkTypewriter = (btn, fullText) => {
            const text = String(fullText || '');
            btn.textContent = '';
            const state = { i: 0 };
            const tick = () => {
                const chunk = text.slice(0, state.i);
                btn.textContent = chunk;
                if (state.i >= text.length) return false;
                state.i = Math.min(text.length, state.i + Math.max(2, Math.ceil(text.length / 40)));
                return true;
            };
            tick();
            const interval = setInterval(() => {
                if (!tick()) clearInterval(interval);
            }, 30);
            this._interviewSuggestionsStream.timers.push(interval);
        };

        normalized.forEach((item, idx) => {
            const delay = idx * 120;
            const t = setTimeout(() => {
                if (suggestionType === 'both') {
                    if (!item.target.trim() && !item.translation.trim()) return;
                    const li = document.createElement('li');
                    li.className = 'suggestion-chip-row';
                    li.dataset.face = 'target';
                    const btn = document.createElement('button');
                    btn.type = 'button';
                    btn.className = 'suggestion-chip';

                    const del = document.createElement('button');
                    del.type = 'button';
                    del.className = 'suggestion-chip-delete';
                    del.title = 'Remove suggestion';
                    del.setAttribute('aria-label', 'Remove suggestion');
                    del.innerHTML = '×';
                    del.addEventListener('click', (e) => {
                        e.stopPropagation();
                        this._renderInterviewSuggestions(this._interviewSuggestionsItems.filter((it) => it.id !== item.id));
                    });

                    const toggle = document.createElement('button');
                    toggle.type = 'button';
                    toggle.className = 'suggestion-chip-lang';
                    toggle.title = 'Switch language';
                    toggle.setAttribute('aria-label', 'Switch language');
                    toggle.innerHTML =
                        '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M17 1l4 4-4 4"/><path d="M3 11V9a4 4 0 0 1 4-4h14"/><path d="M7 23l-4-4 4-4"/><path d="M21 13v2a4 4 0 0 1-4 4H3"/></svg>';
                    toggle.addEventListener('click', (e) => {
                        e.stopPropagation();
                        const nextFace = li.dataset.face === 'translation' ? 'target' : 'translation';
                        li.dataset.face = nextFace;
                        const full = this._suggestionFaceText(item, nextFace);
                        mkTypewriter(btn, full);
                    });

                    btn.addEventListener('click', () => {
                        const face = li.dataset.face === 'translation' ? 'translation' : 'target';
                        const text = this._suggestionFaceText(item, face);
                        if (!text.trim()) return;
                        const ta = document.getElementById('chat-input');
                        this._pickedSuggestion = { id: item.id, text };
                        if (ta) this._insertIntoTextarea(ta, `${text} `);
                    });

                    const timeSpan = document.createElement('span');
                    timeSpan.className = 'suggestion-chip-time';
                    timeSpan.textContent = renderTime;

                    li.appendChild(btn);
                    li.appendChild(del);
                    li.appendChild(timeSpan);
                    li.appendChild(toggle);
                    list.appendChild(li);

                    mkTypewriter(btn, this._suggestionFaceText(item, 'target'));
                    return;
                }

                const text = this._suggestionChipLabel(item, suggestionType);
                if (!text.trim()) return;
                const li = document.createElement('li');
                li.className = 'suggestion-chip-row';
                const btn = document.createElement('button');
                btn.type = 'button';
                btn.className = 'suggestion-chip';
                btn.addEventListener('click', () => {
                    const ta = document.getElementById('chat-input');
                    this._pickedSuggestion = { id: item.id, text };
                    if (ta) this._insertIntoTextarea(ta, `${text} `);
                });
                const del = document.createElement('button');
                del.type = 'button';
                del.className = 'suggestion-chip-delete';
                del.title = 'Remove suggestion';
                del.setAttribute('aria-label', 'Remove suggestion');
                del.innerHTML = '×';
                del.addEventListener('click', (e) => {
                    e.stopPropagation();
                    this._renderInterviewSuggestions(this._interviewSuggestionsItems.filter((it) => it.id !== item.id));
                });

                const timeSpan = document.createElement('span');
                timeSpan.className = 'suggestion-chip-time';
                timeSpan.textContent = renderTime;

                li.appendChild(btn);
                li.appendChild(del);
                li.appendChild(timeSpan);
                list.appendChild(li);
                mkTypewriter(btn, text);
            }, delay);
            this._interviewSuggestionsStream.timers.push(t);
        });

        this._dockInterviewSuggestionsRight();
    }
,


    async _runInterviewSuggestions(gen, { transcriptContext, userDraft }) {
        if (gen !== this._interviewSuggestGen) return;
        const panel = document.getElementById('interview-suggestions-panel');
        try {
            // If we didn't get a speaker/draft marker for some reason, start timing here.
            if (!this._interviewSuggestPerf.t0) this._markInterviewSuggestStart(null);
            this._llmCalls = (this._llmCalls || 0) + 1;
            // Local lexical excerpts first — skips the embed+Pinecone chain.
            // Empty result → backend falls back to Pinecone if configured.
            let contextSnippets = [];
            try {
                const excerpts = await invoke('select_context_excerpts', {
                    userId: this._getInterviewUserId(),
                    query: [transcriptContext, userDraft].filter(Boolean).join('\n'),
                    maxChars: 3000,
                });
                contextSnippets = (excerpts || []).map((e) => `[${e.docType}] ${e.text}`);
            } catch { /* excerpt lookup is best-effort; Pinecone fallback covers it */ }
            const res = await invoke('suggest_interview_answers', {
                req: {
                    userId: this._getInterviewUserId(),
                    transcriptContext: transcriptContext || null,
                    userDraft: userDraft || null,
                    contextSnippets,
                },
            });
            if (gen !== this._interviewSuggestGen) return;
            this._renderInterviewSuggestionsStream(res.suggestions || []);
            this._markInterviewSuggestDone(true);
        } catch (e) {
            console.warn('[Interview] suggest', e);
            if (gen === this._interviewSuggestGen) this._updateSuggestionsEmptyState();
            this._markInterviewSuggestDone(false);
        }
    }
,


    _normalizeInterviewSuggestionItems(raw) {
        const st = settingsManager.get().suggestion_type || 'translation';
        if (!Array.isArray(raw)) return [];
        return raw.map((x, i) => {
            if (typeof x === 'string') {
                if (st === 'translation') return { id: i, target: '', translation: x, suggestion_kind: 'answer' };
                if (st === 'target') return { id: i, target: x, translation: '', suggestion_kind: 'answer' };
                return { id: i, target: x, translation: x, suggestion_kind: 'answer' };
            }
            const id = typeof x.id === 'number' ? x.id : i;
            return {
                id,
                target: x.target != null ? String(x.target) : '',
                translation: x.translation != null ? String(x.translation) : '',
                suggestion_kind: x.suggestionKind != null
                    ? String(x.suggestionKind)
                    : (x.suggestion_kind != null ? String(x.suggestion_kind) : 'answer'),
            };
        });
    }
,


    _suggestionFaceText(item, face) {
        return face === 'translation' ? item.translation : item.target;
    }
,


    _suggestionChipLabel(item, suggestionType) {
        if (suggestionType === 'translation') return item.translation || item.target;
        if (suggestionType === 'target') return item.target || item.translation;
        return item.target || item.translation;
    }
,


    _renderInterviewSuggestions(items) {
        const panel = document.getElementById('interview-suggestions-panel');
        const list = document.getElementById('interview-suggestions-list');
        if (!panel || !list) return;
        const normalized = this._normalizeInterviewSuggestionItems(items);
        this._interviewSuggestionsItems = normalized.slice();
        if (this._interviewSuggestionsClosed) {
            panel.style.display = '';
            this._setRightPanelCollapsed(true);
            this._updateSuggestionsEmptyState();
            return;
        }
        if (!this._isSuggestionsMode()) {
            panel.style.display = 'none';
            list.innerHTML = '';
            this._undockInterviewSuggestions();
            return;
        }
        if (!normalized.length) {
            panel.style.display = '';
            list.innerHTML = '';
            this._dockInterviewSuggestionsRight();
            this._setRightPanelCollapsed(true);
            this._updateSuggestionsEmptyState();
            return;
        }
        this._updateSuggestionsEmptyState();
        this._setRightPanelCollapsed(false);
        panel.style.display = '';
        list.innerHTML = '';
        const suggestionType = settingsManager.get().suggestion_type || 'translation';
        normalized.forEach((item) => {
            if (suggestionType === 'both') {
                if (!item.target.trim() && !item.translation.trim()) return;
                const li = document.createElement('li');
                li.className = 'suggestion-chip-row';
                li.dataset.face = 'target';
                const btn = document.createElement('button');
                btn.type = 'button';
                btn.className = 'suggestion-chip';
                btn.textContent = this._suggestionFaceText(item, 'target');

                const del = document.createElement('button');
                del.type = 'button';
                del.className = 'suggestion-chip-delete';
                del.title = 'Remove suggestion';
                del.setAttribute('aria-label', 'Remove suggestion');
                del.innerHTML = '×';
                del.addEventListener('click', (e) => {
                    e.stopPropagation();
                    this._renderInterviewSuggestions(this._interviewSuggestionsItems.filter((it) => it.id !== item.id));
                });

                const toggle = document.createElement('button');
                toggle.type = 'button';
                toggle.className = 'suggestion-chip-lang';
                toggle.title = 'Switch language';
                toggle.setAttribute('aria-label', 'Switch language');
                toggle.innerHTML =
                    '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M17 1l4 4-4 4"/><path d="M3 11V9a4 4 0 0 1 4-4h14"/><path d="M7 23l-4-4 4-4"/><path d="M21 13v2a4 4 0 0 1-4 4H3"/></svg>';
                toggle.addEventListener('click', (e) => {
                    e.stopPropagation();
                    const nextFace = li.dataset.face === 'translation' ? 'target' : 'translation';
                    li.dataset.face = nextFace;
                    btn.textContent = this._suggestionFaceText(item, nextFace);
                });
                btn.addEventListener('click', () => {
                    const face = li.dataset.face === 'translation' ? 'translation' : 'target';
                    const text = this._suggestionFaceText(item, face);
                    if (!text.trim()) return;
                    const ta = document.getElementById('chat-input');
                    this._pickedSuggestion = { id: item.id, text };
                    if (ta) this._insertIntoTextarea(ta, `${text} `);
                });
                li.appendChild(btn);
                li.appendChild(del);
                li.appendChild(toggle);
                list.appendChild(li);
                return;
            }
            const text = this._suggestionChipLabel(item, suggestionType);
            if (!text.trim()) return;
            const li = document.createElement('li');
            li.className = 'suggestion-chip-row';
            const btn = document.createElement('button');
            btn.type = 'button';
            btn.className = 'suggestion-chip';
            btn.textContent = text;
            btn.addEventListener('click', () => {
                const ta = document.getElementById('chat-input');
                this._pickedSuggestion = { id: item.id, text };
                if (ta) this._insertIntoTextarea(ta, `${text} `);
            });
            const del = document.createElement('button');
            del.type = 'button';
            del.className = 'suggestion-chip-delete';
            del.title = 'Remove suggestion';
            del.setAttribute('aria-label', 'Remove suggestion');
            del.innerHTML = '×';
            del.addEventListener('click', (e) => {
                e.stopPropagation();
                this._renderInterviewSuggestions(this._interviewSuggestionsItems.filter((it) => it.id !== item.id));
            });
            li.appendChild(btn);
            li.appendChild(del);
            list.appendChild(li);
        });

        // For Interview template, show suggestions in a split right panel (subtitle stays visible on the left).
        this._dockInterviewSuggestionsRight();
    }
,

};
