/**
 * App — main application controller
 * Wires together: settings, UI, Soniox client, and audio capture
 */

import { settingsManager } from './settings.js';
import { TranscriptUI } from './ui.js';
import { sonioxClient, sonioxMicClient } from './soniox.js';
import { elevenLabsTTS } from './elevenlabs-tts.js';
import { googleTTS } from './google-tts.js';
import { edgeTTSRust } from './edge-tts.js';
import { audioPlayer } from './audio-player.js';
import { updater } from './updater.js';

const { invoke } = window.__TAURI__.core;
const { getCurrentWindow } = window.__TAURI__.window;
const { listen } = window.__TAURI__.event;

import { settingsFormMethods } from './settings-form.js';
import { ttsMethods } from './tts.js';
import { sessionMethods } from './session.js';
import { conversationMethods } from './conversations.js';
import { windowMethods } from './window.js';
import { updaterMethods } from './updater-ui.js';
import { shortcutMethods } from './shortcuts.js';
import { interviewPanelMethods } from './interview-panel.js';

class App {
    constructor() {
        this.isRunning = false;
        this.isStarting = false; // Guard against re-entry
        this.currentSource = 'system'; // 'system' | 'microphone' | 'both'
        this.translationMode = 'soniox'; // 'soniox' | 'local'
        this.transcriptUI = null;
        this.appWindow = getCurrentWindow();
        this.localPipelineChannel = null;
        this.localPipelineReady = false;
        this.recordingStartTime = null;
        this.sessionStartTime = null;  // Session start timestamp (new Date())
        this.sessionSourceLang = 'auto';
        this.sessionTargetLang = 'vi';
        this.sessionMode = 'one_way';
        this.ttsEnabled = false;  // TTS runtime toggle
        this.isPinned = true;     // Always-on-top state
        this.sidebarOpen = false; // Sidebar toggle state (always starts closed)
        this.sessionActive = false;    // true once a session has been started via start() or +New
        this.readOnlyMode = false;     // true when viewing a past conversation
        this.activeConversationFilename = null;
        this._chipTimer = null;        // 1s interval driving the REC timer chip
        this._lastSavedAt = null;      // set when a session save succeeds

        // Chat UI (UI-only) — input posts into subtitle timeline
        this.currentTemplate = null; // 'Interview' | 'Meeting' | null
        this._interviewCvFile = null;
        this._interviewJdFile = null;
        this._interviewSuggestTimer = null;
        this._interviewSuggestGen = 0;
        this._ingestInterviewDebounce = null;
        this._suggestionsDock = {
            originalParent: null,
            originalNextSibling: null,
            docked: false,
        };
        this._interviewSuggestionsClosed = false;
        this._interviewSuggestionsItems = [];
        this._pickedSuggestion = null;
        this._lastInterviewSuggestArgs = { transcriptContext: null, userDraft: null };
        this._rightPanelCollapsed = false;
        this._interviewSuggestPerf = {
            origin: null, // 'speaker' | 'draft' | null
            t0: 0,
            timer: null,
            hideTimer: null,
        };
        this._interviewSuggestionsStream = {
            timers: [],
        };
        this._brainstormPending = false;
        this._permissions = null;      // cached result of check_permissions (probed once per launch)
        this._settingsSnapshot = null; // JSON snapshot of the settings form for dirty detection
        this._sessionFilename = null;  // transcript file backing the current session (autosave overwrites)
        this._savedSessionJson = null; // sessionLog snapshot at last successful save — dirty detection
    }

    async init() {
        // Load settings
        await settingsManager.load();

        // Set version from Tauri
        try {
            const ver = await window.__TAURI__.app.getVersion();
            const el = document.getElementById('about-version');
            if (el && ver) el.textContent = `v${ver}`;
        } catch { /* non-fatal */ }

        // Init transcript UI
        const transcriptContainer = document.getElementById('transcript-content');
        this.transcriptUI = new TranscriptUI(transcriptContainer);
        this.transcriptUI.onChange = () => this._updateSessionChip();
        this.transcriptUI.onAfterRender = () => this._injectBrainstormButton();

        // Check platform — hide Local MLX on non-Apple-Silicon
        await this._checkPlatformSupport();

        // Rewrite ⌘-style tooltips for non-mac keyboards
        this._applyShortcutHints();

        // Apply saved settings to UI
        this._applySettings(settingsManager.get());

        // Bind event listeners
        this._bindEvents();

        // Bind keyboard shortcuts
        this._bindKeyboardShortcuts();

        // Subscribe to settings changes
        settingsManager.onChange((settings) => this._applySettings(settings));

        // Init audio player for TTS
        audioPlayer.init();

        // Wire TTS audio callbacks for providers that use audioPlayer
        for (const tts of [elevenLabsTTS, edgeTTSRust, googleTTS]) {
            tts.onAudioChunk = (base64Audio, isFinal) => {
                audioPlayer.enqueue(base64Audio);
            };
        }
        for (const tts of [elevenLabsTTS, edgeTTSRust, googleTTS]) {
            tts.onError = (error) => {
                console.error('[TTS]', error);
                this._showToast(error, 'error');
            };
        }

        // Window position restore disabled — causes issues on Retina displays
        // await this._restoreWindowPosition();

        // Check for updates (non-blocking)
        this._initAboutTab();
        this._checkForUpdates();

        // Load sidebar conversation list
        this._loadConversationList();

        try {
            const v = await window.__TAURI__.app.getVersion();
            console.log(`🌐 MyJavis v${v} initialized`);
        } catch {
            console.log('🌐 MyJavis initialized');
        }
    }

    // ─── Event Binding ──────────────────────────────────────

    _bindEvents() {
        // Sidebar toggle
        document.getElementById('btn-toggle-sidebar').addEventListener('click', () => {
            this._toggleSidebar();
        });

        // Mobile: close sidebar when tapping backdrop
        document.getElementById('mobile-overlay-backdrop')?.addEventListener('click', () => {
            if (!this.isMobile) return;
            // Close whichever overlay is open
            this._setMobileSheetOpen(false);
            this.sidebarOpen = false;
            document.body.classList.remove('sidebar-open');
            document.getElementById('sidebar')?.classList.add('hidden');
        });

        // Mobile FAB: toggle suggestions bottom sheet
        document.getElementById('btn-toggle-right-panel')?.addEventListener('click', () => {
            if (!this.isMobile) return;
            if (!this._isSuggestionsMode()) return;
            const open = !document.body.classList.contains('sheet-open');
            this._setMobileSheetOpen(open);
        });

        // Mobile: auto-collapse bottom sheet when user scrolls transcript
        document.getElementById('transcript-container')?.addEventListener('scroll', () => {
            if (!this.isMobile) return;
            if (!document.body.classList.contains('sheet-open')) return;
            this._setMobileSheetOpen(false);
        }, { passive: true });

        // New conversation = finalize this session, then reset (save-first —
        // a failed save aborts the reset so the transcript survives in memory).
        document.getElementById('btn-new-conversation').addEventListener('click', () => {
            this._startNewSessionFlow();
        });

        // Quick "+" in the control bar — same new-session flow, no sidebar needed
        document.getElementById('btn-quick-new')?.addEventListener('click', () => {
            this._startNewSessionFlow();
        });

        // Read-only banner: return from history view to the live session
        document.getElementById('btn-back-to-live')?.addEventListener('click', () => {
            this._startNewSessionFlow();
        });

        // Aa button: toggle the transcript text-size/color controls
        document.getElementById('btn-display-controls')?.addEventListener('click', () => {
            const el = document.getElementById('display-controls');
            if (el) el.classList.toggle('visible');
        });

        // ⋮ More menu (copy / open folder) — click to toggle, click-outside closes
        const moreMenu = document.getElementById('more-menu');
        document.getElementById('btn-more')?.addEventListener('click', (e) => {
            e.stopPropagation();
            moreMenu?.classList.toggle('open');
        });
        document.addEventListener('click', (e) => {
            if (moreMenu?.classList.contains('open')
                && (!moreMenu.contains(e.target) || e.target.closest('.menu-item'))) {
                moreMenu.classList.remove('open');
            }
        });

        // Settings button
        document.getElementById('btn-settings').addEventListener('click', () => {
            this._showView('settings');
        });

        // Back from settings — confirm if the form has unsaved edits
        document.getElementById('btn-back').addEventListener('click', () => {
            this._closeSettings();
        });


        // Close button (overlay)
        document.getElementById('btn-close').addEventListener('click', async () => {
            await this._saveWindowPosition();
            await this.stop();
            await this._finalizeSession(); // save transcript before quitting
            await this.appWindow.close();
        });

        // Minimize button
        document.getElementById('btn-minimize').addEventListener('click', async () => {
            await this._saveWindowPosition();
            await this.appWindow.minimize();
        });

        // Pin/Unpin button
        document.getElementById('btn-pin').addEventListener('click', () => {
            this._togglePin();
        });

        // Font size quick controls
        document.getElementById('btn-font-up').addEventListener('click', () => this._adjustFontSize(4));
        document.getElementById('btn-font-down').addEventListener('click', () => this._adjustFontSize(-4));

        // Color dot controls
        document.querySelectorAll('.color-dot').forEach(dot => {
            dot.addEventListener('click', () => {
                document.querySelectorAll('.color-dot').forEach(d => d.classList.remove('active'));
                dot.classList.add('active');
                const color = dot.dataset.color;
                this.transcriptUI.configure({ fontColor: color });
                settingsManager.save({ font_color: color }).catch(() => {});
            });
        });

        this._initInterviewUploads();
        this._bindInterviewSettingsKeys();
        this._bindDimChips();

        // Interview suggestions triggered by inline brainstorm button (see _injectBrainstormButton)

        // Close Interview suggestions panel
        document.getElementById('btn-close-suggestions')?.addEventListener('click', () => {
            this._interviewSuggestionsClosed = true;
            // Collapse instead of fully hiding so the "Suggestions" open button
            // stays in the same header position as the close button.
            const panel = document.getElementById('interview-suggestions-panel');
            if (panel) panel.style.display = '';
            if (this.isMobile) {
                this._setMobileSheetOpen(false);
            } else {
                this._setRightPanelCollapsed(true);
            }
            // Keep docked if it was docked.
        });

        // Regenerate suggestions
        document.getElementById('btn-regenerate-suggestions')?.addEventListener('click', () => {
            if (!this._isSuggestionsMode()) return;
            this._interviewSuggestionsClosed = false;
            this._setRightPanelCollapsed(false);
            const { transcriptContext, userDraft } = this._lastInterviewSuggestArgs || {};
            this._markInterviewSuggestStart('manual');
            this._scheduleSuggestions({ transcriptContext, userDraft });
        });

        // Open suggestions panel (after closing)
        document.getElementById('btn-open-suggestions')?.addEventListener('click', () => {
            if (!this._isSuggestionsMode()) return;
            this._interviewSuggestionsClosed = false;
            if (this.isMobile) {
                this._setMobileSheetOpen(true);
            } else {
                this._setRightPanelCollapsed(false);
            }
            if (this._interviewSuggestionsItems.length) return;
            const { transcriptContext, userDraft } = this._lastInterviewSuggestArgs || {};
            // Manual mode: do not auto-generate on open
            //this._setInterviewSuggestionsStatus('Ready — click ⟳ to generate');
        });

        // Start/Stop button
        document.getElementById('btn-start').addEventListener('click', async () => {
            if (this.isStarting) return; // Prevent re-entry
            try {
                if (this.isRunning) {
                    await this.stop();
                } else {
                    this.isStarting = true;
                    if (!this.sessionActive) {
                        this._createNewSession();
                    }
                    await this.start();
                }
            } catch (err) {
                console.error('[App] Start/Stop error:', err);
                this._showToast(`Error: ${err}`, 'error');
                this.isRunning = false;
                this._updateStartButton();
                this._updateStatus('error');
                this.transcriptUI.clear();
                this.transcriptUI.showPlaceholder();
            } finally {
                this.isStarting = false;
            }
        });

        // Source buttons
        document.getElementById('btn-source-system').addEventListener('click', () => {
            this._setSource('system');
        });

        document.getElementById('btn-source-mic').addEventListener('click', () => {
            this._setSource('microphone');
        });
        document.getElementById('btn-source-both').addEventListener('click', () => {
            this._setSource('both');
        });

        // Copy transcript button
        document.getElementById('btn-copy').addEventListener('click', async () => {
            const text = this.transcriptUI.getPlainText();
            if (text) {
                await navigator.clipboard.writeText(text);
                this._showToast('Copied to clipboard', 'success');
            } else {
                this._showToast('Nothing to copy', 'info');
            }
        });

        // Chat input: Enter sends, Shift+Enter newline
        document.getElementById('chat-input')?.addEventListener('keydown', (e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault();
                this._sendChatMessage();
            }
        });

        // Open saved transcripts folder (kept for Finder access)
        document.getElementById('btn-open-transcripts').addEventListener('click', async () => {
            try {
                await invoke('open_transcript_dir');
            } catch (err) {
                this._showToast('Failed to open folder: ' + err, 'error');
            }
        });

        // Settings form elements
        this._bindSettingsForm();

        // Manual drag for settings view
        // data-tauri-drag-region doesn't work well when parent contains buttons
        // Using Tauri's recommended appWindow.startDragging() approach instead
        document.getElementById('settings-view')?.addEventListener('mousedown', (e) => {
            const interactive = e.target.closest('button, input, select, label, a, textarea, .settings-section, .settings-actions');
            if (!interactive && e.buttons === 1) {
                e.preventDefault();
                this.appWindow.startDragging();
            }
        });

        // Toggle API key visibility
        document.getElementById('btn-toggle-key').addEventListener('click', () => {
            const input = document.getElementById('input-api-key');
            input.type = input.type === 'password' ? 'text' : 'password';
        });

        // Translation mode toggle
        document.getElementById('select-translation-mode').addEventListener('change', (e) => {
            this._updateModeUI(e.target.value);
        });

        // Translation type toggle (one-way / two-way)
        document.getElementById('select-translation-type')?.addEventListener('change', (e) => {
            this._updateTranslationTypeUI(e.target.value);
        });

        // Soniox link
        document.getElementById('link-soniox').addEventListener('click', (e) => {
            e.preventDefault();
            window.__TAURI__.opener.openUrl('https://console.soniox.com/signup/');
        });

        // ElevenLabs link
        document.getElementById('link-elevenlabs')?.addEventListener('click', (e) => {
            e.preventDefault();
            window.__TAURI__.opener.openUrl('https://elevenlabs.io/app/sign-up');
        });

        // Save settings — both top and bottom buttons
        document.getElementById('btn-save-settings').addEventListener('click', () => {
            this._saveSettingsFromForm();
        });
        document.getElementById('btn-save-settings-top')?.addEventListener('click', () => {
            this._saveSettingsFromForm();
        });

        // Slider live updates
        document.getElementById('range-opacity').addEventListener('input', (e) => {
            document.getElementById('opacity-value').textContent = `${e.target.value}%`;
        });

        document.getElementById('range-font-size').addEventListener('input', (e) => {
            document.getElementById('font-size-value').textContent = `${e.target.value}px`;
        });

        document.getElementById('range-max-lines').addEventListener('input', (e) => {
            document.getElementById('max-lines-value').textContent = e.target.value;
        });

        document.getElementById('range-endpoint-delay')?.addEventListener('input', (e) => {
            document.getElementById('endpoint-delay-value').textContent = `${(e.target.value / 1000).toFixed(1)}s`;
        });

        // Toggle ElevenLabs API key visibility
        document.getElementById('btn-toggle-elevenlabs-key')?.addEventListener('click', () => {
            const input = document.getElementById('input-elevenlabs-key');
            input.type = input.type === 'password' ? 'text' : 'password';
        });

        document.getElementById('btn-toggle-google-key')?.addEventListener('click', () => {
            const input = document.getElementById('input-google-tts-key');
            input.type = input.type === 'password' ? 'text' : 'password';
        });

        document.querySelectorAll('.btn-toggle-ai-key').forEach(btn => {
            btn.addEventListener('click', () => {
                const input = document.getElementById(btn.dataset.target);
                if (input) input.type = input.type === 'password' ? 'text' : 'password';
            });
        });

        // Settings tab switching
        document.querySelectorAll('.settings-tab').forEach(tab => {
            tab.addEventListener('click', () => {
                document.querySelectorAll('.settings-tab').forEach(t => t.classList.remove('active'));
                document.querySelectorAll('.settings-tab-content').forEach(c => c.classList.remove('active'));
                tab.classList.add('active');
                document.getElementById(tab.dataset.tab)?.classList.add('active');
            });
        });

        // TTS enable/disable toggle in settings — show/hide detail
        document.getElementById('check-tts-enabled')?.addEventListener('change', (e) => {
            const detail = document.getElementById('tts-settings-detail');
            if (detail) detail.style.display = e.target.checked ? '' : 'none';
        });

        // TTS provider toggle — show/hide relevant settings panels
        document.getElementById('select-tts-provider')?.addEventListener('change', (e) => {
            this._updateTTSProviderUI(e.target.value);
        });

        // TTS speed slider — show value
        document.getElementById('range-tts-speed')?.addEventListener('input', (e) => {
            const label = document.getElementById('tts-speed-value');
            if (label) label.textContent = e.target.value + 'x';
        });

        // Edge TTS speed slider
        document.getElementById('range-edge-speed')?.addEventListener('input', (e) => {
            const label = document.getElementById('edge-speed-value');
            const v = parseInt(e.target.value);
            if (label) label.textContent = (v >= 0 ? '+' : '') + v + '%';
        });

        document.getElementById('range-google-speed')?.addEventListener('input', (e) => {
            const label = document.getElementById('google-speed-value');
            if (label) label.textContent = parseFloat(e.target.value).toFixed(1) + 'x';
        });

        // Add translation term row
        document.getElementById('btn-add-term')?.addEventListener('click', () => {
            this._addTermRow('', '');
        });

        // Add general context row
        document.getElementById('btn-add-general')?.addEventListener('click', () => {
            this._addGeneralRow('', '');
        });

        // TTS toggle button in overlay
        document.getElementById('btn-tts').addEventListener('click', () => {
            this._toggleTTS();
        });

        // Wire Soniox callbacks — the system client is the primary stream
        // (interviewer). The mic client only feeds recognition of the
        // candidate's own speech in split-capture mode; it never triggers
        // suggestions and never drives the main status dot.
        this._sourceClients = { system: sonioxClient, mic: sonioxMicClient };
        this._wireSonioxClient(sonioxClient, 'system');
        this._wireSonioxClient(sonioxMicClient, 'mic');
    }

    _wireSonioxClient(client, source) {
        const isPrimary = source === 'system';

        client.onOriginal = (text, speaker, language) => {
            this.transcriptUI.addOriginal(text, speaker, language, source);
        };

        client.onTranslation = (text) => {
            this.transcriptUI.addTranslation(text, source);
            if (isPrimary) {
                this._speakIfEnabled(text);
            }
            this._onInterviewSpeakerFinal(text, source);
        };

        client.onProvisional = (text, speaker, language) => {
            if (text) {
                if (isPrimary) this._brainstormPending = false;
                this.transcriptUI.setProvisional(text, speaker, language, source);
            } else {
                this.transcriptUI.clearProvisional(source);
            }
        };

        client.onStatusChange = (status) => {
            if (isPrimary) {
                this._updateStatus(status);
            } else if (status === 'error' && this.isRunning) {
                this._showToast('Mic stream lost — auto-hold unavailable', 'error');
            }
        };

        client.onError = (error) => {
            if (isPrimary) {
                this._showToast(error, 'error');
            } else if (typeof error === 'string' && !error.startsWith('Reconnecting')) {
                this._showToast(`Mic: ${error}`, 'error');
            }
        };

        client.onConfidence = (avgConfidence) => {
            if (isPrimary) this.transcriptUI.setConfidence(avgConfidence);
        };
    }

    // ─── Views ──────────────────────────────────────────────

    _showView(view) {
        document.getElementById('overlay-view').classList.toggle('active', view === 'overlay');
        document.getElementById('settings-view').classList.toggle('active', view === 'settings');

        if (view === 'settings') {
            this._populateSettingsForm();
            this._settingsSnapshot = this._snapshotSettingsForm();
        }
    }

    // Rewrite mac-style shortcut glyphs in tooltips for the host platform
    // (⌘ only exists on macOS; Windows/Linux see Ctrl+…).
    _applyShortcutHints() {
        const isMac = navigator.platform.toUpperCase().includes('MAC');
        const mod = isMac ? '⌘' : 'Ctrl';
        const startBtn = document.getElementById('btn-start');
        if (startBtn) startBtn.title = `Start/Stop (${mod}+Enter)`;
        if (isMac) return;
        document.querySelectorAll('[title]').forEach((el) => {
            if (el.title.includes('⌘')) el.title = el.title.replaceAll('⌘', 'Ctrl+');
        });
    }

    _showToast(message, type = 'success') {
        // Remove existing toast
        const existing = document.querySelector('.toast');
        if (existing) existing.remove();

        const toast = document.createElement('div');
        toast.className = `toast ${type}`;
        toast.textContent = message;
        document.body.appendChild(toast);

        // Trigger animation
        requestAnimationFrame(() => {
            toast.classList.add('show');
        });

        // Auto-remove (longer for errors)
        const duration = type === 'error' ? 5000 : 3000;
        setTimeout(() => {
            toast.classList.remove('show');
            setTimeout(() => toast.remove(), 300);
        }, duration);
    }

    _insertIntoTextarea(textarea, insertText) {
        const start = textarea.selectionStart ?? textarea.value.length;
        const end = textarea.selectionEnd ?? textarea.value.length;
        const before = textarea.value.slice(0, start);
        const after = textarea.value.slice(end);
        textarea.value = before + insertText + after;
        const nextPos = start + insertText.length;
        textarea.focus();
        textarea.setSelectionRange(nextPos, nextPos);
        textarea.dispatchEvent(new Event('input', { bubbles: true }));
    }
}

// Methods split into sibling modules are merged onto the prototype here.
Object.assign(App.prototype, settingsFormMethods, ttsMethods, sessionMethods, conversationMethods, windowMethods, updaterMethods, shortcutMethods, interviewPanelMethods);

// Initialize on DOM ready
document.addEventListener('DOMContentLoaded', () => {
    const app = new App();
    app.init();
});
