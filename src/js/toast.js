// Toast notifications + persistent error pill.
// Extracted from app.js — merged onto App.prototype via Object.assign.

export const toastMethods = {

    _showToast(message, type = 'success', { record = true } = {}) {
        // Errors also stick to the ⚠ pill in the status area — a 5s toast is
        // too easy to miss when the failure matters (mic lost, LLM 401…).
        if (type === 'error' && record) {
            this._lastError = { message: String(message), at: Date.now() };
            this._updateErrorPill();
        }

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
    },

    /// Persistent error surface: ⚠ pill holds the last error until the user
    /// dismisses it or a new session starts. Click → full message + dismiss.
    _updateErrorPill() {
        const pill = document.getElementById('error-pill');
        if (!pill) return;
        if (!this._lastError) {
            pill.style.display = 'none';
            return;
        }
        pill.style.display = '';
        pill.title = `${this._lastError.message}\n\nClick to dismiss`;
        if (!pill._bound) {
            pill._bound = true;
            pill.addEventListener('click', () => {
                const msg = this._lastError?.message;
                this._lastError = null;
                this._updateErrorPill();
                if (msg) this._showToast(msg, 'error', { record: false });
            });
        }
    },

    _clearError() {
        this._lastError = null;
        this._updateErrorPill();
    },
};
