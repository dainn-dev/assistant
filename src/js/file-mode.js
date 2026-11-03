// File mode: pick an audio/video file → Deepgram transcript → LLM translation
// → two .srt files next to the input. Merged onto App.prototype via Object.assign.

import { invoke, Channel } from './ipc.js';

const MEDIA_EXTENSIONS = ['mp3', 'm4a', 'mp4', 'm4v', 'mov', 'aac', 'wav', 'flac', 'ogg', 'oga', 'mkv', 'caf', 'aiff'];

/** @param {{ stage: string, current?: number, total?: number }} p @returns {string|null} */
export function fileProgressLabel({ stage, current, total }) {
    switch (stage) {
        case 'decoding': return 'Reading audio…';
        case 'transcribing': return 'Transcribing with Deepgram…';
        case 'translating': return `Translating subtitles… ${current}/${total}`;
        case 'writing': return 'Saving subtitles…';
        default: return null;
    }
}

export const fileModeMethods = {

    async _pickFileForSubtitles() {
        const dialog = window.__TAURI__?.dialog;
        if (!dialog?.open) {
            this._showToast('File picker unavailable', 'error');
            return;
        }
        const selected = await dialog.open({
            multiple: false,
            filters: [{ name: 'Audio / Video', extensions: MEDIA_EXTENSIONS }],
        });
        if (selected) await this._transcribeFile(String(selected));
    }
,

    async _transcribeFile(path) {
        if (this._fileJobRunning) {
            this._showToast('A file is already being processed — wait for it to finish.', 'error');
            return;
        }
        this._fileJobRunning = true;
        const channel = new Channel();
        channel.onmessage = (p) => {
            const label = fileProgressLabel(p);
            if (label) this._showToast(label, 'success', { record: false });
        };
        try {
            const res = await invoke('transcribe_media_file', { path, channel });
            const minutes = Math.round(res.audioSeconds / 60);
            this._showToast(
                `Subtitles ready (${res.cues} lines, ~${minutes} min): ${res.translatedSrt}`,
                'success',
            );
        } catch (err) {
            this._showToast(String(err), 'error');
        } finally {
            this._fileJobRunning = false;
        }
    }
,
};
