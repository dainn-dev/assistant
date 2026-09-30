// Settings form population, save, term/general rows, provider UIs
// Extracted from app.js — methods are merged onto App.prototype via Object.assign.

import { settingsManager } from './settings.js';
import { audioPlayer } from './audio-player.js';
import { updater } from './updater.js';

const { invoke } = window.__TAURI__.core;

// Single source for all language selects — previously ~200 lines of duplicated
// <option> markup in index.html, and two-way A/B only offered the "Popular" subset.
const LANG_NAMES = {
    vi: 'Vietnamese', en: 'English', ja: 'Japanese', ko: 'Korean', zh: 'Chinese',
    fr: 'French', de: 'German', es: 'Spanish', th: 'Thai', id: 'Indonesian',
    af: 'Afrikaans', sq: 'Albanian', ar: 'Arabic', az: 'Azerbaijani', eu: 'Basque',
    be: 'Belarusian', bn: 'Bengali', bs: 'Bosnian', bg: 'Bulgarian', ca: 'Catalan',
    hr: 'Croatian', cs: 'Czech', da: 'Danish', nl: 'Dutch', et: 'Estonian',
    fi: 'Finnish', gl: 'Galician', el: 'Greek', gu: 'Gujarati', he: 'Hebrew',
    hi: 'Hindi', hu: 'Hungarian', it: 'Italian', kn: 'Kannada', kk: 'Kazakh',
    lv: 'Latvian', lt: 'Lithuanian', mk: 'Macedonian', ms: 'Malay', ml: 'Malayalam',
    mr: 'Marathi', no: 'Norwegian', fa: 'Persian', pl: 'Polish', pt: 'Portuguese',
    pa: 'Punjabi', ro: 'Romanian', ru: 'Russian', sr: 'Serbian', sk: 'Slovak',
    sl: 'Slovenian', sw: 'Swahili', sv: 'Swedish', tl: 'Tagalog', ta: 'Tamil',
    te: 'Telugu', tr: 'Turkish', uk: 'Ukrainian', ur: 'Urdu', cy: 'Welsh',
};
const POPULAR_LANGS = ['en', 'ja', 'ko', 'zh', 'vi', 'fr', 'de', 'es', 'th', 'id'];
const ALL_LANG_CODES = Object.keys(LANG_NAMES);

export const settingsFormMethods = {

    /// Build the four language <select>s once from LANG_NAMES.
    /// Two-way A/B selects get the same full list as Source/Target.
    _initLanguageSelects() {
        if (this._langSelectsBuilt) return;
        this._langSelectsBuilt = true;

        const addGroup = (sel, label, codes) => {
            const group = document.createElement('optgroup');
            group.label = label;
            for (const code of codes) {
                const opt = document.createElement('option');
                opt.value = code;
                opt.textContent = LANG_NAMES[code];
                group.appendChild(opt);
            }
            sel.appendChild(group);
        };
        const rest = ALL_LANG_CODES.filter(c => !POPULAR_LANGS.includes(c));

        for (const [id, auto] of [
            ['select-source-lang', true],
            ['select-target-lang', false],
            ['select-lang-a', false],
            ['select-lang-b', false],
        ]) {
            const sel = document.getElementById(id);
            if (!sel) continue;
            sel.innerHTML = '';
            if (auto) {
                const opt = document.createElement('option');
                opt.value = 'auto';
                opt.textContent = 'Auto-detect';
                sel.appendChild(opt);
            }
            addGroup(sel, 'Popular', POPULAR_LANGS);
            addGroup(sel, 'All Languages', rest);
        }
    }
,

    _filterTtsProviders() {
        // Android: only keep Edge + Google (hide ElevenLabs + any future desktop-only providers).
        const select = document.getElementById('select-tts-provider');
        if (!select) return;

        const allowed = new Set(['edge', 'google']);
        Array.from(select.querySelectorAll('option')).forEach((opt) => {
            const val = opt.getAttribute('value') || '';
            if (!allowed.has(val)) opt.remove();
        });

        const s = settingsManager.get();
        const provider = s.tts_provider || 'edge';
        if (!allowed.has(provider)) {
            // Update UI immediately; persist best-effort.
            select.value = 'edge';
            this._updateTTSProviderUI('edge');
            settingsManager.save({ tts_provider: 'edge' }).catch(() => {});
        }

        // Also hide the removed provider settings blocks (if present).
        const el = document.getElementById('tts-elevenlabs-settings');
        if (el) el.style.display = 'none';
    }
,


    _bindSettingsForm() {
        // These are handled in _populateSettingsForm and _saveSettingsFromForm
    }
,


    // ─── Settings Form ─────────────────────────────────────

    /// Serialize every control in the settings view — dynamic rows without
    /// ids are keyed by position so edits there still register as dirty.
    _snapshotSettingsForm() {
        const data = {};
        document
            .querySelectorAll('#settings-view input, #settings-view select, #settings-view textarea')
            .forEach((el, i) => {
                const key = el.id || `__idx${i}`;
                data[key] = el.type === 'checkbox' || el.type === 'radio' ? el.checked : el.value;
            });
        return JSON.stringify(data);
    }
,

    _settingsFormDirty() {
        return !!this._settingsSnapshot && this._settingsSnapshot !== this._snapshotSettingsForm();
    }
,

    /// Leave the settings view, confirming if there are unsaved edits.
    /// Shared by the Back button and the Escape shortcut.
    _closeSettings() {
        if (this._settingsFormDirty() && !confirm('Discard unsaved settings changes?')) {
            return;
        }
        this._settingsSnapshot = null;
        this._showView('overlay');
    }
,

    /// Show live capture-permission status inside the Translation tab.
    async _refreshPermissionHint() {
        const el = document.getElementById('perm-status-hint');
        if (!el) return;
        try {
            const p = await invoke('check_permissions');
            this._permissions = p;
            const mark = (v) => (v === 'granted' ? '✓' : v === 'denied' ? '✗' : '?');
            el.textContent =
                `Capture permissions — system audio: ${mark(p.screen_recording)} ${p.screen_recording} · ` +
                `microphone: ${mark(p.microphone)} ${p.microphone}`;
            el.style.display = '';
        } catch { /* probe unavailable — hide the hint */ }
    }
,

    _populateSettingsForm() {
        this._initLanguageSelects();
        const s = settingsManager.get();

        document.getElementById('input-api-key').value = s.soniox_api_key || '';
        document.getElementById('select-source-lang').value = s.source_language || 'auto';
        document.getElementById('select-target-lang').value = s.target_language || 'vi';
        document.getElementById('select-translation-mode').value = s.translation_mode || 'soniox';
        this._updateModeUI(s.translation_mode || 'soniox');

        // Translation type (one-way / two-way)
        const translationType = s.translation_type || 'one_way';
        document.getElementById('select-translation-type').value = translationType;
        this._updateTranslationTypeUI(translationType);

        // Two-way language selects
        document.getElementById('select-lang-a').value = s.language_a || 'ja';
        document.getElementById('select-lang-b').value = s.language_b || 'vi';

        // Strict language detection
        document.getElementById('check-strict-lang').checked = s.language_hints_strict || false;

        // Endpoint delay
        const endpointDelay = s.endpoint_delay || 3000;
        const delaySlider = document.getElementById('range-endpoint-delay');
        if (delaySlider) delaySlider.value = endpointDelay;
        const delayValue = document.getElementById('endpoint-delay-value');
        if (delayValue) delayValue.textContent = `${(endpointDelay / 1000).toFixed(1)}s`;

        // Audio source radio
        const radioValue = s.audio_source || 'system';
        const radio = document.querySelector(`input[name="audio-source"][value="${radioValue}"]`);
        if (radio) radio.checked = true;

        // Display
        const opacityPercent = Math.round((s.overlay_opacity || 0.85) * 100);
        document.getElementById('range-opacity').value = opacityPercent;
        document.getElementById('opacity-value').textContent = `${opacityPercent}%`;

        document.getElementById('range-font-size').value = s.font_size || 16;
        document.getElementById('font-size-value').textContent = `${s.font_size || 16}px`;

        const fontSel = document.getElementById('select-font-family');
        if (fontSel) fontSel.value = s.font_family || 'Inter';

        document.getElementById('range-max-lines').value = s.max_lines || 5;
        document.getElementById('max-lines-value').textContent = s.max_lines || 5;

        document.getElementById('check-show-original').checked = s.show_original !== false;

        // Custom context (rich format)
        const ctx = s.custom_context;
        // General context rows
        const generalList = document.getElementById('context-general-list');
        if (generalList) {
            generalList.innerHTML = '';
            const generalPairs = ctx?.general || [];
            generalPairs.forEach(g => this._addGeneralRow(g.key, g.value));
        }
        // Transcription terms
        const termsInput = document.getElementById('input-context-terms');
        if (termsInput) {
            termsInput.value = (ctx?.terms || []).join('\n');
        }
        // Background text
        const textInput = document.getElementById('input-context-text');
        if (textInput) {
            textInput.value = ctx?.text || '';
        }
        // Load translation terms as rows
        const termsList = document.getElementById('translation-terms-list');
        if (termsList) {
            termsList.innerHTML = '';
            const terms = ctx?.translation_terms || [];
            terms.forEach(t => this._addTermRow(t.source, t.target));
        }

        // TTS settings
        document.getElementById('input-elevenlabs-key').value = s.elevenlabs_api_key || '';
        document.getElementById('select-tts-voice').value = s.tts_voice_id || '21m00Tcm4TlvDq8ikWAM';
        // Edge TTS settings
        const edgeVoiceSelect = document.getElementById('select-edge-voice');
        if (edgeVoiceSelect) edgeVoiceSelect.value = s.edge_tts_voice || 'vi-VN-HoaiMyNeural';
        const edgeSpeedSlider = document.getElementById('range-edge-speed');
        const edgeSpeedLabel = document.getElementById('edge-speed-value');
        const edgeSpeed = s.edge_tts_speed !== undefined ? s.edge_tts_speed : 20;
        if (edgeSpeedSlider) edgeSpeedSlider.value = edgeSpeed;
        if (edgeSpeedLabel) edgeSpeedLabel.textContent = (edgeSpeed >= 0 ? '+' : '') + edgeSpeed + '%';

        // Google TTS settings
        const googleKeyInput = document.getElementById('input-google-tts-key');
        if (googleKeyInput) googleKeyInput.value = s.google_tts_api_key || '';
        const googleVoiceSelect = document.getElementById('select-google-voice');
        if (googleVoiceSelect) googleVoiceSelect.value = s.google_tts_voice || 'vi-VN-Chirp3-HD-Aoede';
        const googleSpeedSlider = document.getElementById('range-google-speed');
        const googleSpeedLabel = document.getElementById('google-speed-value');
        const googleSpeed = s.google_tts_speed || 1.0;
        if (googleSpeedSlider) googleSpeedSlider.value = googleSpeed;
        if (googleSpeedLabel) googleSpeedLabel.textContent = googleSpeed + 'x';

        // Interview AI fields
        const pineHost = document.getElementById('interview-pinecone-host');
        if (pineHost) pineHost.value = s.pinecone_host || '';
        const pineDim = document.getElementById('interview-pinecone-dim');
        if (pineDim) {
            pineDim.value = String(s.pinecone_vector_dimension ?? 1536);
            this._updateDimChips(pineDim.value);
        }
        const pineKey = document.getElementById('pinecone-api-key');
        if (pineKey) pineKey.value = s.pinecone_api_key || '';
        const llmUrl = document.getElementById('llm-url');
        if (llmUrl) llmUrl.value = s.llm_url || '';
        const llmModel = document.getElementById('llm-model');
        if (llmModel) llmModel.value = s.llm_model || '';
        const llmKey = document.getElementById('llm-api-key');
        if (llmKey) llmKey.value = s.llm_api_key || '';
        const appMode = document.getElementById('select-app-mode');
        if (appMode) appMode.value = s.app_mode || '';
        const suggestionType = document.getElementById('select-suggestion-type');
        if (suggestionType) {
            const v = s.suggestion_type || 'translation';
            suggestionType.value = ['target', 'translation', 'both'].includes(v) ? v : 'translation';
        }
        const earlyEl = document.getElementById('check-early-suggestions');
        if (earlyEl) earlyEl.checked = s.early_suggestions === true;

        // TTS provider
        const providerSelect = document.getElementById('select-tts-provider');
        if (providerSelect) {
            providerSelect.value = s.tts_provider || 'edge';
            this._updateTTSProviderUI(providerSelect.value);
        }

        void this._refreshPermissionHint();
    }
,


    async _saveSettingsFromForm() {
        const settings = {
            soniox_api_key: document.getElementById('input-api-key').value.trim(),
            source_language: document.getElementById('select-source-lang').value,
            target_language: document.getElementById('select-target-lang').value,
            translation_mode: document.getElementById('select-translation-mode').value,
            translation_type: document.getElementById('select-translation-type')?.value || 'one_way',
            language_a: document.getElementById('select-lang-a')?.value || 'ja',
            language_b: document.getElementById('select-lang-b')?.value || 'vi',
            language_hints_strict: document.getElementById('check-strict-lang')?.checked || false,
            endpoint_delay: parseInt(document.getElementById('range-endpoint-delay')?.value || 3000),
            audio_source: document.querySelector('input[name="audio-source"]:checked')?.value || 'system',
            overlay_opacity: parseInt(document.getElementById('range-opacity').value) / 100,
            font_size: parseInt(document.getElementById('range-font-size').value),
            font_family: document.getElementById('select-font-family')?.value || 'Inter',
            font_color: settingsManager.get().font_color || '#ffffff',
            max_lines: parseInt(document.getElementById('range-max-lines').value),
            show_original: document.getElementById('check-show-original').checked,
            custom_context: null,
        };

        // Parse custom context (rich format)
        // General key-value pairs
        const generalPairs = [];
        document.querySelectorAll('#context-general-list .general-row').forEach(row => {
            const key = row.querySelector('.general-key')?.value.trim();
            const value = row.querySelector('.general-value')?.value.trim();
            if (key && value) generalPairs.push({ key, value });
        });

        // Transcription terms
        const termsRaw = document.getElementById('input-context-terms')?.value.trim() || '';
        const terms = termsRaw ? termsRaw.split('\n').map(t => t.trim()).filter(t => t) : [];

        // Background text
        const contextText = document.getElementById('input-context-text')?.value.trim() || '';

        // Translation terms
        const translationTerms = [];
        document.querySelectorAll('#translation-terms-list .term-row').forEach(row => {
            const source = row.querySelector('.term-source')?.value.trim();
            const target = row.querySelector('.term-target')?.value.trim();
            if (source && target) translationTerms.push({ source, target });
        });

        if (generalPairs.length > 0 || terms.length > 0 || contextText || translationTerms.length > 0) {
            settings.custom_context = {
                general: generalPairs,
                terms: terms,
                text: contextText || null,
                translation_terms: translationTerms,
            };
        }

        // TTS settings
        settings.tts_provider = document.getElementById('select-tts-provider')?.value || 'edge';
        settings.elevenlabs_api_key = document.getElementById('input-elevenlabs-key').value.trim();
        settings.tts_voice_id = document.getElementById('select-tts-voice').value;
        settings.edge_tts_voice = document.getElementById('select-edge-voice')?.value || 'vi-VN-HoaiMyNeural';
        settings.edge_tts_speed = parseInt(document.getElementById('range-edge-speed')?.value || 20);
        settings.tts_speed = parseFloat(document.getElementById('range-tts-speed')?.value || 1.2);
        settings.google_tts_api_key = document.getElementById('input-google-tts-key')?.value.trim() || '';
        settings.google_tts_voice = document.getElementById('select-google-voice')?.value || 'vi-VN-Chirp3-HD-Aoede';
        settings.google_tts_speed = parseFloat(document.getElementById('range-google-speed')?.value || 1.0);
        settings.tts_enabled = false;

        settings.pinecone_host = document.getElementById('interview-pinecone-host')?.value?.trim() || '';
        settings.pinecone_vector_dimension = parseInt(
            document.getElementById('interview-pinecone-dim')?.value || '1536',
            10,
        );
        settings.llm_url = document.getElementById('llm-url')?.value?.trim() || '';
        settings.llm_model = document.getElementById('llm-model')?.value?.trim() || '';
        settings.pinecone_api_key = document.getElementById('pinecone-api-key')?.value?.trim() || '';
        settings.llm_api_key = document.getElementById('llm-api-key')?.value?.trim() || '';
        const st = document.getElementById('select-suggestion-type')?.value || 'translation';
        settings.suggestion_type = ['target', 'translation', 'both'].includes(st) ? st : 'translation';
        const am = document.getElementById('select-app-mode')?.value || '';
        settings.app_mode = am === 'Interview' ? am : '';
        settings.early_suggestions = document.getElementById('check-early-suggestions')?.checked === true;

        try {
            await settingsManager.save(settings);
            // Enabling early suggestions kills live TTS — system audio would
            // transcribe the app's own narration back into the transcript.
            if (settings.early_suggestions && this.ttsEnabled) {
                this.ttsEnabled = false;
                this._getActiveTTS().disconnect();
                audioPlayer.stop();
                this._updateTTSButton();
                this._showToast('TTS narration turned off — early suggestions is active', 'success');
            }
            this._settingsSnapshot = null; // saved — Back/Escape must not prompt
            this._showToast('Settings saved', 'success');
            this._showView('overlay');
        } catch (err) {
            this._showToast(`Failed to save: ${err}`, 'error');
        }
    }
,


    // ─── Apply Settings ────────────────────────────────────

    _updateChatInputState() {
        const isInterview = this.currentTemplate === 'Interview';
        const panel = document.getElementById('chat-panel');
        if (panel) panel.style.display = isInterview ? '' : 'none';
        const input = document.getElementById('chat-input');
        if (!input) return;
        if (!isInterview) return;
        const hasKey = !!(settingsManager.get().llm_api_key?.trim());
        input.disabled = !hasKey;
        input.style.display = '';
        input.placeholder = hasKey
            ? 'Type a message… (Enter to send, Shift+Enter for newline)'
            : 'Add LLM API key in Settings to enable chat';
    }
,


    _addTermRow(source = '', target = '') {
        const list = document.getElementById('translation-terms-list');
        if (!list) return;
        const row = document.createElement('div');
        row.className = 'term-row';
        row.innerHTML = `<input type="text" class="term-source" value="${this._escAttr(source)}" placeholder="Source" />` +
            `<input type="text" class="term-target" value="${this._escAttr(target)}" placeholder="Target" />` +
            `<button type="button" class="btn-remove-term" title="Remove">×</button>`;
        row.querySelector('.btn-remove-term').addEventListener('click', () => row.remove());
        list.appendChild(row);
    }
,


    _addGeneralRow(key = '', value = '') {
        const list = document.getElementById('context-general-list');
        if (!list) return;
        const row = document.createElement('div');
        row.className = 'general-row';
        row.innerHTML = `<input type="text" class="general-key" value="${this._escAttr(key)}" placeholder="Key (e.g. domain)" />` +
            `<input type="text" class="general-value" value="${this._escAttr(value)}" placeholder="Value (e.g. Medical)" />` +
            `<button type="button" class="btn-remove-general" title="Remove">×</button>`;
        row.querySelector('.btn-remove-general').addEventListener('click', () => row.remove());
        list.appendChild(row);
    }
,


    _escAttr(str) {
        return str.replace(/"/g, '&quot;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
    }
,


    _updateTTSProviderUI(provider) {
        const ed = document.getElementById('tts-edge-settings');
        const go = document.getElementById('tts-google-settings');
        const el = document.getElementById('tts-elevenlabs-settings');
        if (ed) ed.style.display = provider === 'edge' ? '' : 'none';
        if (go) go.style.display = provider === 'google' ? '' : 'none';
        if (el) el.style.display = provider === 'elevenlabs' ? '' : 'none';
        // Update hint text
        const hint = document.getElementById('tts-provider-hint');
        if (hint) {
            const hints = {
                edge: 'Free, natural voices — no API key needed',
                google: 'Near-human quality — requires Google Cloud API key (1M chars/month free)',
                elevenlabs: 'Premium quality — requires ElevenLabs API key',
            };
            hint.textContent = hints[provider] || '';
        }
    }
,


    _updateTranslationTypeUI(type) {
        const oneway = document.getElementById('section-oneway-langs');
        const twoway = document.getElementById('section-twoway-langs');
        const hintTwoway = document.getElementById('hint-twoway');
        const strictLang = document.getElementById('section-strict-lang');

        if (type === 'two_way') {
            if (oneway) oneway.style.display = 'none';
            if (twoway) twoway.style.display = 'flex';
            if (hintTwoway) hintTwoway.style.display = 'block';
            // Hide strict lang in two-way mode (both languages are specified)
            if (strictLang) strictLang.style.display = 'none';
            // Force-disable TTS in two-way mode to prevent audio feedback loop
            if (this.ttsEnabled) {
                this.ttsEnabled = false;
                this._getActiveTTS().disconnect();
                audioPlayer.stop();
            }
            this._updateTTSButton();
        } else {
            if (oneway) oneway.style.display = 'flex';
            if (twoway) twoway.style.display = 'none';
            if (hintTwoway) hintTwoway.style.display = 'none';
            if (strictLang) strictLang.style.display = 'flex';
            this._updateTTSButton();
        }
    }
,


    _initAboutTab() {
        // GitHub links
        document.getElementById('link-github')?.addEventListener('click', (e) => {
            e.preventDefault();
            window.__TAURI__?.opener?.openUrl('https://github.com/dainn-dev/assistant');
        });
        document.getElementById('link-issues')?.addEventListener('click', (e) => {
            e.preventDefault();
            window.__TAURI__?.opener?.openUrl('https://github.com/dainn-dev/assistant/issues');
        });

        // Check for Updates button
        document.getElementById('btn-check-update')?.addEventListener('click', () => {
            this._triggerUpdateCheck();
        });

        // Download & Install button
        document.getElementById('btn-do-update')?.addEventListener('click', async () => {
            const btnText = document.getElementById('update-btn-text');
            const btn = document.getElementById('btn-do-update');
            const progressDiv = document.getElementById('update-progress');
            const progressFill = document.getElementById('update-progress-fill');
            const progressPct = document.getElementById('update-progress-pct');

            if (btn) btn.disabled = true;
            if (btnText) btnText.textContent = 'Downloading...';
            if (progressDiv) progressDiv.style.display = '';

            try {
                await updater.downloadAndInstall((downloaded, total) => {
                    if (total > 0) {
                        const pct = Math.round((downloaded / total) * 100);
                        if (progressFill) progressFill.style.width = `${pct}%`;
                        if (progressPct) progressPct.textContent = `${pct}%`;
                        if (btnText) btnText.textContent = `Downloading ${pct}%...`;
                    }
                });
                // Install succeeded! Try to restart
                if (btnText) btnText.textContent = 'Restarting...';
                try {
                    const relaunch = window.__TAURI__?.process?.relaunch;
                    if (relaunch) {
                        await relaunch();
                    } else {
                        const invoke = window.__TAURI__?.core?.invoke;
                        if (invoke) await invoke('plugin:process|restart');
                    }
                } catch (restartErr) {
                    // Restart failed (e.g. process plugin not available) but update IS installed
                    console.warn('[Update] Restart failed, update is installed:', restartErr);
                    if (btnText) btnText.textContent = '✅ Updated! Restart app';
                    const statusText = document.getElementById('update-status-text');
                    if (statusText) statusText.textContent = '✅ Update installed — close and reopen the app';
                    if (btn) btn.disabled = true;
                }
            } catch (err) {
                const errMsg = err?.message || String(err);
                if (btnText) btnText.textContent = 'Failed — try again';
                const statusText = document.getElementById('update-status-text');
                if (statusText) statusText.textContent = `⚠️ Install error: ${errMsg}`;
                if (btn) btn.disabled = false;
                console.error('[Update]', err);
            }
        });
    }
,


    _updateDimChips(currentVal) {
        document.querySelectorAll('.ai-dim-chip').forEach(chip => {
            chip.classList.toggle('active', chip.dataset.dim === String(currentVal));
        });
    }
,


    _bindDimChips() {
        const dimInput = document.getElementById('interview-pinecone-dim');
        if (!dimInput) return;
        document.querySelectorAll('.ai-dim-chip').forEach(chip => {
            chip.addEventListener('click', () => {
                dimInput.value = chip.dataset.dim;
                this._updateDimChips(chip.dataset.dim);
            });
        });
        dimInput.addEventListener('input', () => this._updateDimChips(dimInput.value));
    }
,


    _bindInterviewSettingsKeys() {
        document.querySelectorAll('.interview-key-row').forEach((row) => {
            const provider = row.dataset.provider;
            if (!provider) return;
            const keyInput = row.querySelector('.interview-key-input');
            const icon = row.querySelector('.interview-key-icon');

            // Clear input when focused if showing placeholder dots
            keyInput?.addEventListener('focus', () => {
                if (keyInput.dataset.hasSavedKey === 'true') keyInput.value = '';
            });

            // Auto-save on blur
            keyInput?.addEventListener('blur', async () => {
                const apiKey = keyInput.value.trim();
                if (!apiKey || apiKey === '••••••••') return;
                try {
                    await invoke('interview_set_api_key', { payload: { provider, apiKey } });
                    await this._refreshInterviewKeyRows();
                } catch (err) {
                    this._showToast(`Key save failed: ${err}`, 'error');
                }
            });

            // Auto-save on Enter
            keyInput?.addEventListener('keydown', (e) => {
                if (e.key === 'Enter') keyInput.blur();
            });

            // Click green tick → clear
            icon?.addEventListener('click', async () => {
                if (!icon.classList.contains('is-saved')) return;
                try {
                    await invoke('interview_clear_api_key', { provider });
                    await this._refreshInterviewKeyRows();
                } catch (err) {
                    this._showToast(`Clear failed: ${err}`, 'error');
                }
            });
        });
    }
,


    async _refreshInterviewKeyRows() {
        const st = await invoke('interview_key_status').catch((e) => {
            console.warn('[Interview] key status', e);
            return null;
        });
        if (!st) return;
        document.querySelectorAll('.interview-key-row').forEach((row) => {
            const p = row.dataset.provider;
            const input = row.querySelector('.interview-key-input');
            const icon = row.querySelector('.interview-key-icon');
            if (!p) return;
            const on = st[p] === true;
            if (input) {
                input.dataset.hasSavedKey = on ? 'true' : 'false';
                input.classList.toggle('is-saved', on);
                if (on && !input.value) input.value = '••••••••';
                if (!on) input.value = '';
            }
            if (icon) {
                icon.classList.toggle('is-saved', on);
                icon.classList.toggle('not-set', !on);
                icon.title = on ? 'Click to clear' : 'Not set';
                icon.innerHTML = on
                    ? '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><circle cx="12" cy="12" r="10"/><polyline points="9 12 11 14 15 10"/></svg>'
                    : '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><circle cx="12" cy="12" r="10"/><line x1="15" y1="9" x2="9" y2="15"/><line x1="9" y1="9" x2="15" y2="15"/></svg>';
            }
        });
    }
,

};
