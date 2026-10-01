// Update check flow and the update-available toast
// Extracted from app.js — methods are merged onto App.prototype via Object.assign.

import { updater } from './updater.js';


export const updaterMethods = {

    async _checkForUpdates() {
        updater.onUpdateFound = (version, notes) => {
            this._onUpdateAvailable(version, notes);
        };
        updater.onError = (err) => {
            this._updateCheckFailed = true;
            const raw = String(err?.message || err);
            // A 404/invalid-JSON from the release endpoint means no release
            // exists yet — that's an expected state, not a broken check.
            const friendly = /valid release json|404|not found/i.test(raw)
                ? 'No release published yet'
                : `Check failed: ${raw}`;
            const statusText = document.getElementById('update-status-text');
            if (statusText) statusText.textContent = `⚠️ ${friendly}`;
            // Startup checks fail silently (offline, no release yet) — only a
            // manual click surfaces the persistent error pill.
            if (this._manualUpdateCheck) {
                this._manualUpdateCheck = false;
                this._showToast?.(friendly, 'error');
            }
        };
        updater.onCheckComplete = (hasUpdate) => {
            this._manualUpdateCheck = false;
            const checkBtn = document.getElementById('btn-check-update');
            if (checkBtn) checkBtn.classList.remove('spinning');
            if (!hasUpdate && !this._pendingUpdateVersion) {
                if (this._updateCheckFailed) {
                    // Keep the error message — don't overwrite with "up to date".
                    this._updateCheckFailed = false;
                    return;
                }
                const statusText = document.getElementById('update-status-text');
                if (statusText) statusText.textContent = '✅ App is up to date';
            }
        };
        this._updateCheckFailed = false;
        // Delay check slightly so app finishes loading first
        setTimeout(() => {
            const statusText = document.getElementById('update-status-text');
            const checkBtn = document.getElementById('btn-check-update');
            if (statusText) statusText.textContent = 'Checking for updates...';
            if (checkBtn) checkBtn.classList.add('spinning');
            updater.checkForUpdates();
        }, 3000);
    }
,


    _triggerUpdateCheck() {
        this._manualUpdateCheck = true;
        const statusText = document.getElementById('update-status-text');
        const checkBtn = document.getElementById('btn-check-update');
        if (statusText) statusText.textContent = 'Checking for updates...';
        if (checkBtn) checkBtn.classList.add('spinning');
        updater.checkForUpdates();
    }
,


    _onUpdateAvailable(version, notes) {
        this._pendingUpdateVersion = version;

        // 1. Show badge on settings gear
        const badge = document.getElementById('settings-badge');
        if (badge) badge.style.display = '';

        // 2. Update About tab status
        const statusEl = document.getElementById('update-status');
        const statusText = document.getElementById('update-status-text');
        const actions = document.getElementById('update-actions');
        if (statusEl) statusEl.classList.add('has-update');
        if (statusText) statusText.textContent = `🆕 Update v${version} available`;
        if (actions) actions.style.display = '';

        // 3. Show subtle hint on main screen
        const existing = document.querySelector('.update-hint');
        if (existing) existing.remove();
        const hint = document.createElement('div');
        hint.className = 'update-hint';
        hint.textContent = `Update v${version} available — go to Settings → About`;
        hint.addEventListener('click', () => {
            this._showView('settings');
            // Switch to About tab
            document.querySelectorAll('.settings-tab').forEach(t => t.classList.remove('active'));
            document.querySelectorAll('.settings-tab-content').forEach(t => t.classList.remove('active'));
            const aboutTab = document.querySelector('[data-tab="tab-about"]');
            const aboutContent = document.getElementById('tab-about');
            if (aboutTab) aboutTab.classList.add('active');
            if (aboutContent) aboutContent.classList.add('active');
            hint.remove();
        });
        document.body.appendChild(hint);

        // Auto-hide hint after 8 seconds
        setTimeout(() => { if (hint.parentNode) hint.remove(); }, 8000);
    }
,

};
