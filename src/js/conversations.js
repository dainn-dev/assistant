// Sidebar session list, read-only transcript view, session meta
// Extracted from app.js — methods are merged onto App.prototype via Object.assign.

import { invoke } from './ipc.js';

export const conversationMethods = {

    // ─── Compact Mode ───────────────────────────────

    _toggleSidebar() {
        this.sidebarOpen = !this.sidebarOpen;
        const sidebar = document.getElementById('sidebar');
        sidebar.classList.toggle('hidden', !this.sidebarOpen);
        if (this.isMobile) {
            document.body.classList.toggle('sidebar-open', this.sidebarOpen);
        }
    }
,


    // ─── Toast ─────────────────────────────────────────────

    // ─── Session History ───────────────────────────────────

    /// Load a saved conversation as a resumable session — the transcript
    /// becomes the live sessionLog and pressing ▶ keeps recording into the
    /// same file. Save-first protects any unsaved draft already in memory.
    async _openConversation(filename) {
        // Opening history while recording would let live segments keep
        // streaming into the loaded view — block instead of corrupting both.
        if (this.isRunning || this.isStarting) {
            this._showToast('Stop the current session to view history', 'error');
            return;
        }
        if (!(await this._finalizeSession())) return;

        this._earlyBumpEpoch?.();
        document.querySelectorAll('#conversation-list .conversation-item').forEach(el => {
            el.classList.toggle('active', el.dataset.filename === filename);
        });

        this.activeConversationFilename = filename;

        const contentEl = document.getElementById('transcript-content');
        if (contentEl) contentEl.textContent = 'Loading...';

        try {
            // Prefer the structured sidecar; fall back to markdown parsing
            // for transcripts saved before sidecars existed.
            let raw = await invoke('read_transcript_segments', { filename });
            let segments = Array.isArray(raw) ? raw : raw?.segments;
            if (!Array.isArray(segments)) {
                const text = await invoke('read_transcript', { filename });
                segments = this._parseSavedTranscriptToSegments(text);
            }
            this._reviewTranscriptKb = Math.ceil(
                segments.reduce((n, s) => n + (s.original || '').length, 0) / 1024
            );
            this._reviewLoadCached?.(filename);
            this.transcriptUI.configure({ viewMode: 'subtitle' });
            this.transcriptUI.clear();
            // Adopt loaded segments as the sessionLog — resumable, and the
            // next save overwrites the same file with the full timeline.
            this.transcriptUI.loadSegments(segments, { replaceSessionLog: true });
            this._sessionFilename = filename;
            this._savedSessionJson = JSON.stringify(this.transcriptUI.sessionLog);
            this._lastSavedAt = Date.now();
            this.sessionActive = true;
            this._updateSessionChip();

            if (!segments.length && contentEl) {
                contentEl.textContent = 'No transcript content.';
            }
        } catch (err) {
            if (contentEl) contentEl.textContent = `Error loading: ${err}`;
        }
    }
,


    _parseSavedTranscriptToSegments(text) {
        const raw = String(text || '');
        if (!raw.trim()) return [];

        // Strip first YAML frontmatter block: --- ... ---
        let body = raw;
        if (body.startsWith('---')) {
            const second = body.indexOf('\n---', 3);
            if (second !== -1) {
                const after = body.indexOf('\n', second + 1);
                body = after !== -1 ? body.slice(after + 1) : '';
            }
        }

        const lines = body.split(/\r?\n/);
        const segments = [];

        let currentSpeaker = null;
        let pending = null; // { speaker, original, createdAt }

        const speakerRe = /^\*\*Speaker\s+(.+?):\*\*\s*$/i;

        const flushPendingIfAny = () => {
            if (!pending) return;
            segments.push({
                original: pending.original || '',
                translation: pending.translation || '',
                status: 'translated',
                speaker: pending.speaker,
                language: null,
                confidence: null,
                createdAt: pending.createdAt,
            });
            pending = null;
        };

        for (let i = 0; i < lines.length; i++) {
            const lineRaw = lines[i];
            const line = (lineRaw || '').trim();
            if (!line) continue;

            const sp = line.match(speakerRe);
            if (sp) {
                const s = (sp[1] || '').trim();
                currentSpeaker = s;
                continue;
            }

            if (line.startsWith('>')) {
                // If we already had a pending EN without a VI, flush it before starting a new one.
                flushPendingIfAny();
                const original = line.replace(/^>\s*/, '').trim();
                pending = {
                    speaker: currentSpeaker,
                    original,
                    translation: '',
                    createdAt: Date.now() + segments.length,
                };
                continue;
            }

            // Treat as VI line if we have pending EN.
            if (pending && !pending.translation) {
                pending.translation = line;
                flushPendingIfAny();
            }
        }

        // Flush any trailing EN without VI
        flushPendingIfAny();

        // Normalize speaker values: if stored as "Speaker 1" accidentally, reduce to "1"
        segments.forEach(s => {
            if (typeof s.speaker === 'string') {
                const m = s.speaker.match(/^Speaker\s+(.+)$/i);
                if (m) s.speaker = m[1].trim();
            }
        });

        return segments.filter(s => (s.original || s.translation));
    }
,


    async _loadConversationList() {
        const listEl = document.getElementById('conversation-list');
        if (!listEl) return;

        try {
            const sessions = await invoke('list_transcripts');
            listEl.innerHTML = '';

            if (!sessions.length) {
                const li = document.createElement('li');
                li.className = 'conversation-empty';
                li.textContent = 'No saved conversations yet — record one and it shows up here.';
                listEl.appendChild(li);
                return;
            }

            sessions.forEach(s => {
                const meta = this._parseSessionMeta(s);
                const li = document.createElement('li');
                li.className = 'conversation-item';
                li.dataset.filename = s.filename;
                const badge = s.has_review
                    ? '<span class="conversation-badge" title="Reviewed">★</span>'
                    : '';
                li.innerHTML = `
                    <span class="conversation-label">🗨 ${meta.date} ${meta.time}${badge}</span>
                    <button type="button" class="btn-review-conversation" title="Review this session with the LLM" aria-label="Review session">✦</button>
                    <button type="button" class="btn-remove-conversation" title="Delete conversation" aria-label="Delete conversation">×</button>
                `;

                const reviewBtn = li.querySelector('.btn-review-conversation');
                reviewBtn.addEventListener('click', async (e) => {
                    e.stopPropagation();
                    await this._openConversation(s.filename);
                    this._reviewGenerate?.(s.filename, false);
                });

                const removeBtn = li.querySelector('.btn-remove-conversation');
                removeBtn.addEventListener('click', async (e) => {
                    e.stopPropagation();
                    const filename = s.filename;
                    const ok = confirm(`Delete conversation "${filename}"?\n\nThis cannot be undone.`);
                    if (!ok) return;
                    try {
                        await invoke('delete_transcript', { filename });
                        // Deleting the currently open file: drop into a fresh
                        // session so the next save doesn't recreate it.
                        if (this._sessionFilename === filename || this.activeConversationFilename === filename) {
                            this._createNewSession();
                            this.activeConversationFilename = null;
                        }
                        await this._loadConversationList();
                        this._showToast('Deleted conversation', 'success');
                    } catch (err) {
                        this._showToast(`Delete failed: ${err}`, 'error');
                    }
                });
                li.addEventListener('click', () => {
                    this._openConversation(s.filename);
                });
                listEl.appendChild(li);
            });
        } catch (err) {
            console.error('[Sidebar] Failed to load conversations:', err);
        }
    }
,


    _parseSessionMeta(session) {
        // created_at format: "2026-03-27 10:21:05"
        const parts = (session.created_at || '').split(' ');
        const date = parts[0] || '';
        const time = parts[1] ? parts[1].slice(0, 5) : '';
        return { date, time, duration: '', langPair: '' };
    }
,


    _formatBytes(bytes) {
        if (bytes < 1024) return `${bytes} B`;
        if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
        return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
    }
,

};
