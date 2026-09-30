// Session lifecycle: start/stop/end, capture, transcript save, status
// Extracted from app.js — methods are merged onto App.prototype via Object.assign.

import { settingsManager } from './settings.js';
import { sonioxClient, sonioxMicClient } from './soniox.js';
import { elevenLabsTTS } from './elevenlabs-tts.js';
import { edgeTTSRust } from './edge-tts.js';
import { audioPlayer } from './audio-player.js';

const { invoke } = window.__TAURI__.core;

export const sessionMethods = {

    async _checkPlatformSupport() {
        try {
            // Check if we're on macOS Apple Silicon
            const arch = await invoke('get_platform_info');
            const info = JSON.parse(arch);
            this.isAppleSilicon = (info.os === 'macos' && info.arch === 'aarch64');
            this.isMobile = (info.os === 'android' || info.os === 'ios');
            this.isAndroid = (info.os === 'android');
        } catch {
            // Fallback: check via navigator
            this.isAppleSilicon = navigator.platform === 'MacIntel' &&
                navigator.userAgent.includes('Mac OS X');
            const ua = (navigator.userAgent || '').toLowerCase();
            this.isAndroid = ua.includes('android');
            this.isMobile = this.isAndroid || /iphone|ipad|ipod/.test(ua);
        }

        if (this.isMobile) {
            document.body.classList.add('mobile');
        }

        // Platform adaptations (Android)
        if (this.isAndroid) {
            await this._applyMobileDefaults();
            this._filterTtsProviders();
        }

        if (!this.isAppleSilicon) {
            // Hide Local MLX option
            const select = document.getElementById('select-translation-mode');
            const localOption = select?.querySelector('option[value="local"]');
            if (localOption) localOption.remove();

            // Force soniox mode if user had local selected
            const settings = settingsManager.get();
            if (settings.translation_mode === 'local') {
                settings.translation_mode = 'soniox';
                settingsManager.save(settings);
            }
        }
    }
,


    async _applyMobileDefaults() {
        // Android: system audio requires MediaProjection; default to mic for a smoother first-run.
        const s = settingsManager.get();
        const src = s.audio_source || 'system';
        if (src === 'system') {
            try {
                await settingsManager.save({ audio_source: 'microphone' });
            } catch {
                // Non-fatal: we'll still continue with runtime defaults.
            }
        }
    }
,


    _applySettings(settings) {
        // Overlay translucency — only the card background fades; text and
        // controls stay fully readable (drives --bg-primary's alpha channel).
        const alpha = Number(settings.overlay_opacity);
        document.documentElement.style.setProperty(
            '--overlay-alpha',
            Number.isFinite(alpha) ? String(Math.min(1, Math.max(0, alpha))) : '0.85',
        );

        // Apply app font size to UI bits that use CSS vars (e.g. Interview suggestions)
        const fs = Number(settings.font_size || 16);
        document.documentElement.style.setProperty('--app-font-size', `${Number.isFinite(fs) ? fs : 16}px`);
        const ff = String(settings.font_family || '').trim();
        if (ff) {
            document.documentElement.style.setProperty('--app-font-family', ff);
        } else {
            document.documentElement.style.removeProperty('--app-font-family');
        }

        // Update transcript UI
        if (this.transcriptUI) {
            this.transcriptUI.configure({
                maxLines: settings.max_lines || 5,
                showOriginal: settings.show_original !== false,
                fontSize: settings.font_size || 16,
                fontColor: settings.font_color || '#ffffff',
                viewMode: 'subtitle',
            });
            // Keep quick-control UI in sync with persisted settings
            const fsDisplay = document.getElementById('font-size-display');
            if (fsDisplay) fsDisplay.textContent = settings.font_size || 16;
            document.querySelectorAll('#display-controls .color-dot').forEach((d) => {
                d.classList.toggle('active', d.dataset.color === settings.font_color);
            });
        }

        // Update current source button states
        this.currentSource = settings.audio_source || 'system';
        this._updateSourceButtons();

        // TTS starts OFF each launch (constructor default). Don't reset it here —
        // onChange fires on every settings save, and clobbering it would silently
        // kill a live TTS toggle (e.g. after saving settings mid-session).
        this._updateTTSButton();

        // Sync mode from settings
        const savedMode = settings.app_mode || null;
        if (savedMode !== this.currentTemplate) {
            this._setTemplateMode(savedMode);
        }

        this._updateChatInputState();
    }
,


    // ─── Source Control ────────────────────────────────────

    _setSource(source) {
        const wasRunning = this.isRunning;
        const labels = { system: 'System Audio', microphone: 'Microphone', both: 'System + Mic' };
        const label = labels[source] || source;

        this.currentSource = source;
        this._updateSourceButtons();
        settingsManager.save({ audio_source: source }).catch(() => {});

        // If currently running, restart with the new source
        if (wasRunning) {
            this._showToast(`Switching to ${label} — restarting capture…`, 'success');
            this.stop().then(() => this.start());
        } else {
            this._showToast(`Source: ${label}`, 'success');
        }
    }
,


    _updateSourceButtons() {
        document.getElementById('btn-source-system').classList.toggle('active',
            this.currentSource === 'system');
        document.getElementById('btn-source-mic').classList.toggle('active',
            this.currentSource === 'microphone');
        document.getElementById('btn-source-both').classList.toggle('active',
            this.currentSource === 'both');
    }
,


    _updateModeUI(mode) {
        const isSoniox = mode === 'soniox';

        // Toggle hints
        const hintSoniox = document.getElementById('hint-mode-soniox');
        const hintLocal = document.getElementById('hint-mode-local');
        if (hintSoniox) hintSoniox.style.display = isSoniox ? '' : 'none';
        if (hintLocal) hintLocal.style.display = !isSoniox ? '' : 'none';

        // Toggle Soniox-only sections
        const sectionApiKey = document.getElementById('section-api-key');
        const sectionContext = document.getElementById('section-soniox-context');
        if (sectionApiKey) sectionApiKey.style.display = isSoniox ? '' : 'none';
        if (sectionContext) sectionContext.style.display = isSoniox ? '' : 'none';
    }
,


    // ─── Start/Stop ────────────────────────────────────────

    async start() {
        if (this.readOnlyMode) {
            this._showToast('Viewing a saved conversation — press + New to go live first', 'error');
            return;
        }

        const settings = settingsManager.get();
        this.translationMode = settings.translation_mode || 'soniox';
        // Never log the settings object — it contains API keys.

        // Check Soniox API key only for cloud mode
        if (this.translationMode === 'soniox' && !settings.soniox_api_key) {
            this._showToast('Soniox API key is required. Add it in Settings.', 'error');
            this._showView('settings');
            return;
        }

        // Check ElevenLabs key only if TTS is enabled AND provider is elevenlabs
        if (this.ttsEnabled && settings.tts_provider === 'elevenlabs' && !settings.elevenlabs_api_key) {
            this._showToast('TTS is ON but ElevenLabs API key is missing. Add it in Settings or disable TTS.', 'error');
            this._showView('settings');
            return;
        }

        // Probe capture permissions once per launch — a denial gets an
        // actionable message here instead of a cryptic mid-stream failure.
        if (!this._permissions) {
            try {
                this._permissions = await invoke('check_permissions');
            } catch {
                this._permissions = { screen_recording: 'unknown', microphone: 'unknown' };
            }
        }
        const needsSystemAudio = this.currentSource === 'system' || this.currentSource === 'both';
        const needsMic = this.currentSource === 'microphone' || this.currentSource === 'both';
        if (needsSystemAudio && this._permissions.screen_recording === 'denied') {
            this._showToast('System audio is blocked — enable Screen Recording for MyJavis in System Settings → Privacy & Security, then retry.', 'error');
            this._updateStatus('error');
            return;
        }
        if (needsMic && this._permissions.microphone === 'denied') {
            this._showToast('No microphone available — connect an input device or check mic privacy settings.', 'error');
            this._updateStatus('error');
            return;
        }

        this.isRunning = true;
        this.sessionActive = true;
        this._updateStartButton();
        this._updateControlsForMode();
        this._updateSessionChip();
        if (!this.recordingStartTime) this.recordingStartTime = Date.now();

        // Record session metadata for auto-save
        if (!this.sessionStartTime) {
            this.sessionStartTime = new Date();
            const translationType = settings.translation_type || 'one_way';
            this.sessionMode = translationType;
            if (translationType === 'two_way') {
                this.sessionSourceLang = settings.language_a || 'ja';
                this.sessionTargetLang = settings.language_b || 'vi';
            } else {
                this.sessionSourceLang = settings.source_language || 'auto';
                this.sessionTargetLang = settings.target_language || 'vi';
            }
        }

        // Clear transcript only if nothing is showing
        if (!this.transcriptUI.hasContent()) {
            this.transcriptUI.showListening();
        } else {
            this.transcriptUI.clearProvisional();
        }

        if (this.translationMode === 'local') {
            await this._startLocalMode(settings);
        } else {
            await this._startSonioxMode(settings);
        }

        // Early-suggestion scheduler polls the system-source transcript
        // while recording — self-gates on the setting inside the tick.
        this._earlyStartTicker?.();

        // Start TTS if enabled
        if (this.ttsEnabled) {
            const tts = this._getActiveTTS();
            this._configureTTS(tts, settings);
            tts.connect();
            audioPlayer.resume();
        }
    }
,


    async _startSonioxMode(settings) {
        // Connect to Soniox
        console.log('[App] Connecting to Soniox...');
        this._updateStatus('connecting');
        const sonioxConfig = {
            apiKey: settings.soniox_api_key,
            sourceLanguage: settings.source_language,
            targetLanguage: settings.target_language,
            customContext: settings.custom_context,
            translationType: settings.translation_type || 'one_way',
            languageA: settings.language_a,
            languageB: settings.language_b,
            languageHintsStrict: settings.language_hints_strict || false,
            endpointDelay: settings.endpoint_delay || 3000,
        };
        sonioxClient.connect(sonioxConfig);

        // Split mode: Interview + System&Mic keeps the two audio
        // sources on independent recognition streams so the app always knows
        // who is speaking (system = interviewer, mic = candidate).
        const useSplit = this.currentSource === 'both' && this._isSuggestionsMode();
        if (useSplit) {
            sonioxMicClient.connect(sonioxConfig);
        }

        // If system audio is selected, request MediaProjection first (no-op on desktop).
        try {
            if (this.currentSource === 'system' || this.currentSource === 'both') {
                await invoke('request_media_projection');
            }

            if (useSplit) {
                await this._startSplitCapture();
            } else {
                let audioChunkCount = 0;

                const channel = new window.__TAURI__.core.Channel();
                channel.onmessage = (pcmData) => {
                    audioChunkCount++;
                    if (audioChunkCount <= 3 || audioChunkCount % 50 === 0) {
                        console.log(`[Audio] Batch #${audioChunkCount}, size:`, pcmData?.length || 0);
                    }
                    // Forward batched audio to Soniox
                    const bytes = new Uint8Array(pcmData);
                    sonioxClient.sendAudio(bytes.buffer);
                };

                console.log('[App] Starting audio capture, source:', this.currentSource);
                await invoke('start_capture', {
                    source: this.currentSource,
                    channel: channel,
                });
                console.log('[App] Audio capture started successfully');
            }
        } catch (err) {
            console.error('Failed to start audio capture:', err);
            this._showToast(`Audio error: ${err}`, 'error');
            await this.stop();
        }
    }
,


    // Split capture: two Rust IPC channels → two Soniox clients. System audio
    // → sonioxClient (primary), mic → sonioxMicClient. If the mic side fails
    // after system is running, we keep going on system-only with a visible
    // warning rather than tearing down the whole session.
    async _startSplitCapture() {
        const systemChannel = new window.__TAURI__.core.Channel();
        systemChannel.onmessage = (pcmData) => {
            const bytes = new Uint8Array(pcmData);
            sonioxClient.sendAudio(bytes.buffer);
        };

        const micChannel = new window.__TAURI__.core.Channel();
        micChannel.onmessage = (pcmData) => {
            const bytes = new Uint8Array(pcmData);
            sonioxMicClient.sendAudio(bytes.buffer);
        };

        try {
            console.log('[App] Starting split capture (system + mic channels)');
            await invoke('start_split_capture', {
                systemChannel,
                micChannel,
            });
            console.log('[App] Split capture started successfully');
        } catch (err) {
            // Rust rolls back system capture when mic fails; if the error names
            // the mic, degrade to system-only so the interviewer side still works.
            if (typeof err === 'string' && err.startsWith('microphone')) {
                console.warn('[App] Mic capture failed, falling back to system-only:', err);
                sonioxMicClient.disconnect();
                this._micDegraded = true;
                try {
                    await invoke('start_capture', { source: 'system', channel: systemChannel });
                    this._showToast('Mic unavailable — interview stream only; auto-hold disabled', 'error');
                    return;
                } catch (sysErr) {
                    throw sysErr;
                }
            }
            throw err;
        }
    }
,


    async _startLocalMode(settings) {
        console.log('[App] Starting Local mode (MLX models)...');
        this._updateStatus('connecting');

        // Step 0: Check audio permission FIRST (before loading models).
        // If system audio is selected, request MediaProjection first (no-op on desktop).
        try {
            if (this.currentSource === 'system' || this.currentSource === 'both') {
                await invoke('request_media_projection');
            }
            await invoke('start_capture', {
                source: this.currentSource,
                channel: new window.__TAURI__.core.Channel(), // dummy channel for permission check
            });
            await invoke('stop_capture');
        } catch (err) {
            console.error('[App] Audio permission check failed:', err);
            this._showToast(`Audio permission required: ${err}`, 'error');
            this.isRunning = false;
            this._updateStartButton();
            this._updateStatus('error');
            this.transcriptUI.clear();
            this.transcriptUI.showPlaceholder();
            return;
        }

        // Step 1: Check if MLX setup is complete
        try {
            const checkResult = await invoke('check_mlx_setup');
            const status = JSON.parse(checkResult);
            if (!status.ready) {
                this._showToast('Setting up MLX models (one-time, ~5GB)...', 'success');
                this.transcriptUI.showStatusMessage('Downloading MLX models (one-time setup)...');
                await this._runMlxSetup();
            }
        } catch (err) {
            console.warn('[App] MLX check failed (proceeding anyway):', err);
        }

        console.log('[App] MLX check passed, starting pipeline...');

        // Step 1: Start pipeline FIRST (independent of audio)
        try {
            this._showToast('Starting local pipeline...', 'success');

            this.localPipelineChannel = new window.__TAURI__.core.Channel();
            this.localPipelineReady = false;

            this.localPipelineChannel.onmessage = (msg) => {
                let data;
                try {
                    data = (typeof msg === 'string') ? JSON.parse(msg) : msg;
                } catch (e) {
                    console.warn('[Local] JSON parse failed:', typeof msg, msg);
                    return;
                }
                try {
                    this._handleLocalPipelineResult(data);
                } catch (e) {
                    console.error('[Local] Handler error for type:', data?.type, e);
                }
            };

            const sourceLangMap = {
                'auto': 'auto', 'ja': 'Japanese', 'en': 'English',
                'zh': 'Chinese', 'ko': 'Korean', 'vi': 'Vietnamese',
            };
            const sourceLang = sourceLangMap[settings.source_language] || 'Japanese';

            await invoke('start_local_pipeline', {
                sourceLang: sourceLang,
                targetLang: settings.target_language || 'vi',
                channel: this.localPipelineChannel,
            });
            console.log('[App] Local pipeline spawned');
        } catch (err) {
            console.error('Failed to start pipeline:', err);
            this._showToast(`Pipeline error: ${err}`, 'error');
            await this.stop();
            return;
        }

        // Step 2: Start audio capture
        try {
            const audioChannel = new window.__TAURI__.core.Channel();
            let audioChunkCount = 0;

            audioChannel.onmessage = async (pcmData) => {
                audioChunkCount++;
                if (audioChunkCount <= 3 || audioChunkCount % 50 === 0) {
                    console.log(`[Local] Audio batch #${audioChunkCount}, size:`, pcmData?.length || 0);
                }
                try {
                    // Raw binary IPC body — avoids ~10x JSON-array bloat on every 200ms chunk.
                    // (Android falls back to a JSON array body handled Rust-side.)
                    await invoke('send_audio_to_pipeline', new Uint8Array(pcmData));
                } catch (e) {
                    // Pipeline may not be ready yet
                }
            };

            await invoke('start_capture', {
                source: this.currentSource,
                channel: audioChannel,
            });
            console.log('[App] Audio capture started');
        } catch (err) {
            console.error('Audio capture failed (pipeline still running):', err);
            this._showToast(`Audio: ${err}. Pipeline still loading...`, 'error');
        }
    }
,


    _handleLocalPipelineResult(data) {
        switch (data.type) {
            case 'ready':
                this.localPipelineReady = true;
                this._updateStatus('connected');
                this.transcriptUI.removeStatusMessage();
                this.transcriptUI.showListening();
                this._showToast('Local models ready!', 'success');
                break;
            case 'result':
                // Chase effect: show original first (gray), then translation (white)
                if (data.original) {
                    this.transcriptUI.addOriginal(data.original);
                }
                // Small delay for visual "chase" effect
                setTimeout(() => {
                if (data.translated) {
                    this.transcriptUI.addTranslation(data.translated);
                    this._speakIfEnabled(data.translated);
                    this._onInterviewSpeakerFinal(data.translated);
                }
                }, 80);
                break;
            case 'status':
                const msg = data.message || 'Loading...';
                // Status bar: show compact message (strip [pipeline] prefix)
                const statusText = document.getElementById('status-text');
                if (statusText) {
                    const compact = msg.replace(/^\[pipeline\]\s*/, '');
                    statusText.textContent = compact;
                }
                // Transcript area: only show loading/starting messages, not debug logs
                if (!msg.startsWith('[pipeline]')) {
                    this.transcriptUI.showStatusMessage(msg);
                }
                break;
            case 'done':
                this._updateStatus('disconnected');
                break;
        }
    }
,


    async _runMlxSetup() {
        const modal = document.getElementById('setup-modal');
        const progressFill = document.getElementById('setup-progress-fill');
        const progressPct = document.getElementById('setup-progress-pct');
        const statusText = document.getElementById('setup-status-text');
        const cancelBtn = document.getElementById('btn-cancel-setup');

        // Step mapping: step name → total progress weight
        const stepWeights = { check: 5, venv: 10, packages: 35, models: 50 };
        let totalProgress = 0;

        const updateStep = (stepName, icon, isActive) => {
            const stepEl = document.getElementById(`step-${stepName}`);
            if (!stepEl) return;
            stepEl.querySelector('.step-icon').textContent = icon;
            stepEl.classList.toggle('active', isActive);
            stepEl.classList.toggle('done', icon === '✅');
        };

        const updateProgress = (pct) => {
            totalProgress = Math.min(100, pct);
            progressFill.style.width = totalProgress + '%';
            progressPct.textContent = Math.round(totalProgress) + '%';
        };

        // Show modal
        modal.style.display = 'flex';

        return new Promise((resolve, reject) => {
            const channel = new window.__TAURI__.core.Channel();

            // Cancel handler
            const onCancel = () => {
                modal.style.display = 'none';
                reject(new Error('Setup cancelled'));
            };
            cancelBtn.addEventListener('click', onCancel, { once: true });

            channel.onmessage = (msg) => {
                let data;
                try {
                    data = (typeof msg === 'string') ? JSON.parse(msg) : msg;
                } catch (e) {
                    return;
                }

                switch (data.type) {
                    case 'progress':
                        statusText.textContent = data.message || 'Working...';

                        // Update step indicators
                        if (data.step) {
                            // Mark previous steps as done
                            const steps = ['check', 'venv', 'packages', 'models'];
                            const currentIdx = steps.indexOf(data.step);
                            steps.forEach((s, i) => {
                                if (i < currentIdx) updateStep(s, '✅', false);
                                else if (i === currentIdx) updateStep(s, '🔄', true);
                            });

                            if (data.done) {
                                updateStep(data.step, '✅', false);
                            }

                            // Calculate overall progress
                            let pct = 0;
                            steps.forEach((s, i) => {
                                if (i < currentIdx) pct += stepWeights[s];
                                else if (i === currentIdx) {
                                    pct += (data.progress || 0) / 100 * stepWeights[s];
                                }
                            });
                            updateProgress(pct);
                        }
                        break;

                    case 'complete':
                        updateProgress(100);
                        statusText.textContent = '✅ ' + (data.message || 'Setup complete!');
                        ['check', 'venv', 'packages', 'models'].forEach(s => updateStep(s, '✅', false));

                        // Close modal after brief delay
                        setTimeout(() => {
                            modal.style.display = 'none';
                            resolve();
                        }, 1000);
                        break;

                    case 'error':
                        statusText.textContent = '❌ ' + (data.message || 'Setup failed');
                        cancelBtn.textContent = 'Close';
                        cancelBtn.removeEventListener('click', onCancel);
                        cancelBtn.addEventListener('click', () => {
                            modal.style.display = 'none';
                            reject(new Error(data.message));
                        }, { once: true });
                        break;

                    case 'log':
                        console.log('[MLX Setup]', data.message);
                        break;
                }
            };

            invoke('run_mlx_setup', { channel })
                .catch(err => {
                    statusText.textContent = '❌ ' + err;
                    modal.style.display = 'none';
                    reject(err);
                });
        });
    }
,


    async _stopCapture() {
        if (!this.isRunning) return;
        this.isRunning = false;
        this._updateStartButton();
        this._updateControlsForMode();
        // Invalidate any in-flight early-suggestion stream, but keep the
        // visible card — stopping is a pause, not a discard.
        this._earlyStopTicker?.();
        this._early?.bumpEpoch();

        try {
            await invoke('stop_capture');
        } catch (err) {
            console.error('Failed to stop audio capture:', err);
        }

        if (this.translationMode === 'local') {
            try {
                await invoke('stop_local_pipeline');
            } catch (err) {
                console.error('Failed to stop local pipeline:', err);
            }
            this.localPipelineReady = false;
            this.transcriptUI.removeStatusMessage();
            this._updateStatus('disconnected');
        } else {
            sonioxClient.disconnect();
            sonioxMicClient.disconnect();
            this._micDegraded = false;
        }

        // Promote in-flight provisional text before clearing — provisional
        // words never reach sessionLog, so a short capture would otherwise
        // silently lose what was on screen.
        for (const [source, slot] of Object.entries(this.transcriptUI.provisionalBySource)) {
            if (slot.text?.trim()) {
                this.transcriptUI.addOriginal(slot.text, slot.speaker, slot.language, source);
            }
        }
        this.transcriptUI.clearProvisional();

        elevenLabsTTS.disconnect();
        edgeTTSRust.disconnect();
        audioPlayer.stop();
        this._updateSessionChip();
    }
,


    async stop() {
        // Stop is a pure pause — no file writes. The transcript persists in
        // memory until the session is finalized via + New or app close
        // (_finalizeSession), which is when the file is created.
        await this._stopCapture();
    }
,


    _createNewSession() {
        this.readOnlyMode = false;
        this._earlyBumpEpoch?.();
        const banner = document.getElementById('readonly-banner');
        if (banner) banner.style.display = 'none';
        this.activeConversationFilename = null;
        this.sessionActive = true;
        this.sessionStartTime = null;
        this.recordingStartTime = null;
        this._sessionFilename = null;
        this._savedSessionJson = null;
        this._lastSavedAt = null;
        if (this._chipTimer) {
            clearInterval(this._chipTimer);
            this._chipTimer = null;
        }
        this._updateControlsForMode();
        this._updateSessionChip();

        this.transcriptUI.clear();
        this.transcriptUI.showPlaceholder();

        document.querySelectorAll('#conversation-list .conversation-item').forEach(el => {
            el.classList.remove('active');
        });
    }
,


    /// Shared "+ New / Back to live" flow: stop if recording, save-first
    /// (never silently discard), then reset into a fresh session.
    async _startNewSessionFlow() {
        if (this.isRunning || this.isStarting) {
            if (!confirm('Stop recording and start a new session?')) return;
            await this.stop();
        }
        if (!(await this._finalizeSession())) return;
        this._createNewSession();
    }
,

    /// Save the current session's transcript if it has unsaved content.
    /// Returns false when a save was attempted and failed — callers should
    /// keep the session alive so the data isn't lost.
    async _finalizeSession() {
        if (!this._isSessionDirty()) return true;
        return this._saveTranscriptFile();
    }
,

    /// Session chip — makes the session lifecycle visible at a glance:
    ///   recording → "● REC m:ss" (pulsing red, ticks every second)
    ///   paused with unsaved content → "⏸ Draft · N lines" (amber)
    ///   just saved → "✓ Saved" (green)
    ///   idle / read-only → hidden
    /// Also keeps the quick-new button label honest ("Save & New" only
    /// when there is something to save, "+ New" otherwise).
    _updateSessionChip() {
        const chip = document.getElementById('session-chip');
        if (!chip) return;
        const newBtn = document.getElementById('btn-quick-new');
        const lines = this.transcriptUI?.sessionLog?.length || 0;
        const hasProvisional = !!this.transcriptUI?.provisionalText?.trim();

        if (this._chipTimer) {
            clearInterval(this._chipTimer);
            this._chipTimer = null;
        }

        if (this.readOnlyMode) {
            chip.style.display = 'none';
        } else if (this.isRunning) {
            const secs = Math.max(0, Math.floor((Date.now() - (this.recordingStartTime || Date.now())) / 1000));
            chip.style.display = '';
            chip.className = 'session-chip recording';
            chip.textContent = `● REC ${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, '0')}`;
            chip.title = `${lines} line${lines === 1 ? '' : 's'} captured so far`;
            this._chipTimer = setInterval(() => this._updateSessionChip(), 1000);
        } else if (lines > 0 || hasProvisional) {
            chip.style.display = '';
            chip.className = 'session-chip draft';
            chip.textContent = `⏸ Draft · ${lines} line${lines === 1 ? '' : 's'}`;
            chip.title = 'Not saved to a file yet — press "+ New" (or close the app) to save';
        } else if (this._lastSavedAt) {
            chip.style.display = '';
            chip.className = 'session-chip saved';
            chip.textContent = '✓ Saved';
            chip.title = 'Session saved to disk';
        } else {
            chip.style.display = 'none';
        }

        if (newBtn) {
            newBtn.textContent = (this.isRunning || lines > 0 || hasProvisional) ? 'Save & New' : '+ New';
        }
    }
,

    _updateStartButton() {
        const btn = document.getElementById('btn-start');
        const iconPlay = document.getElementById('icon-play');
        const iconStop = document.getElementById('icon-stop');

        btn.classList.toggle('recording', this.isRunning);
        iconPlay.style.display = this.isRunning ? 'none' : 'block';
        iconStop.style.display = this.isRunning ? 'block' : 'none';
    }
,


    _updateEndButtonVisibility() {
        this._updateControlsForMode();
    }
,


    _updateControlsForMode() {
        const btnStart = document.getElementById('btn-start');
        if (this.readOnlyMode) {
            btnStart.disabled = true;
            btnStart.style.opacity = '0.35';
            btnStart.style.pointerEvents = 'none';
        } else {
            btnStart.disabled = false;
            btnStart.style.opacity = '';
            btnStart.style.pointerEvents = '';
        }
        // (No separate End Session button — + New finalizes the session.)
    }
,


    // ─── Transcript Persistence ───────────────────────────────

    /// True when sessionLog differs from the last successfully saved state.
    /// JSON compare is only run on user actions (stop/+New), never per-segment.
    _isSessionDirty() {
        if (!this.transcriptUI?.hasSessionContent()) return false;
        return this._savedSessionJson !== JSON.stringify(this.transcriptUI.sessionLog);
    }
,

    _formatDuration(ms) {
        const totalSec = Math.floor(ms / 1000);
        const min = Math.floor(totalSec / 60);
        const sec = totalSec % 60;
        return `${min}m ${sec}s`;
    }
,


    async _saveTranscriptFile() {
        const startMs = this.recordingStartTime || Date.now();
        const durationMs = Date.now() - startMs;
        const duration = this._formatDuration(durationMs);

        // Use session metadata captured at start()
        const sourceLang = this.sessionSourceLang || document.getElementById('select-source-lang')?.value || 'auto';
        const targetLang = this.sessionTargetLang || document.getElementById('select-target-lang')?.value || 'vi';
        const mode = this.sessionMode || 'one_way';

        const content = this.transcriptUI.getFullSessionText({
            model: this.translationMode === 'soniox' ? 'Soniox Cloud API' : 'Local MLX Whisper',
            sourceLang,
            targetLang,
            duration,
            mode,
            audioSource: this.currentSource,
        });

        if (!content) return false;

        try {
            const path = await invoke('save_transcript', {
                content,
                segments: this.transcriptUI.sessionLog,
                filename: this._sessionFilename, // null → new timestamped file
            });
            const filename = path.split(/[\\/]/).pop();
            const isNewFile = !this._sessionFilename;
            this._sessionFilename = filename;
            this._savedSessionJson = JSON.stringify(this.transcriptUI.sessionLog);
            this._lastSavedAt = Date.now();
            this._updateSessionChip();
            this._showToast(`Saved: ${filename}`, 'success');
            // A freshly created file should appear in the sidebar immediately
            if (isNewFile && typeof this._loadConversationList === 'function') {
                void this._loadConversationList();
            }
            return true;
        } catch (err) {
            console.error('Failed to save transcript:', err);
            this._showToast('Failed to save transcript — session kept in memory', 'error');
            return false;
        }
    }
,


    // ─── Status ────────────────────────────────────────────

    _updateStatus(status) {
        const dot = document.getElementById('status-indicator');
        const text = document.getElementById('status-text');

        dot.className = 'status-dot';

        switch (status) {
            case 'connecting':
                dot.classList.add('connecting');
                // Pulsing dot is signal enough — keep the bar clean.
                text.textContent = '';
                break;
            case 'connected':
                dot.classList.add('connected');
                // REC chip already shows the live state — a "Listening"
                // label next to it is redundant.
                text.textContent = this.isRunning ? '' : 'Listening';
                break;
            case 'disconnected':
                dot.classList.add('disconnected');
                text.textContent = (this.transcriptUI?.sessionLog?.length > 0 || this.transcriptUI?.provisionalText)
                    ? 'Paused'
                    : 'Ready';
                break;
            case 'error':
                dot.classList.add('error');
                text.textContent = 'Error';
                break;
        }
    }
,

};
