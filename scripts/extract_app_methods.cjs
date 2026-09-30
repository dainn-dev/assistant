/**
 * One-off extraction: split methods out of `src/js/app.js`'s god class into
 * ES modules that are merged back onto App.prototype via Object.assign.
 * Pure code motion — method bodies are copied verbatim.
 *
 * Usage: node scripts/extract_app_methods.cjs [--write]
 * Default is a dry run (prints plan + coverage). --write emits files.
 */
const fs = require('fs');
const path = require('path');

const JS_DIR = path.join(__dirname, '..', 'src', 'js');
const APP_PATH = path.join(JS_DIR, 'app.js');
const src = fs.readFileSync(APP_PATH, 'utf8');
const lines = src.split('\n');
const WRITE = process.argv.includes('--write');

// ── module assignment: method name -> module file ──────────────────────────
const MODULES = {
  'settings-form.js': {
    obj: 'settingsFormMethods',
    desc: 'Settings form population, save, term/general rows, provider UIs',
    methods: [
      '_filterTtsProviders', '_bindSettingsForm', '_populateSettingsForm',
      '_saveSettingsFromForm', '_updateChatInputState', '_addTermRow',
      '_addGeneralRow', '_escAttr', '_updateTTSProviderUI',
      '_updateTranslationTypeUI', '_updateDimChips', '_bindDimChips',
      '_bindInterviewSettingsKeys', '_refreshInterviewKeyRows', '_initAboutTab',
    ],
  },
  'tts.js': {
    obj: 'ttsMethods',
    desc: 'TTS provider selection, configure/toggle/speak, TTS button state',
    methods: [
      '_toggleTTS', '_getActiveTTS', '_configureTTS', '_updateTTSButton',
      '_speakIfEnabled',
    ],
  },
  'session.js': {
    obj: 'sessionMethods',
    desc: 'Session lifecycle: start/stop/end, capture, transcript save, status',
    methods: [
      '_checkPlatformSupport', '_applyMobileDefaults', '_applySettings',
      '_setSource', '_updateSourceButtons', '_updateModeUI', 'start',
      '_startSonioxMode', '_startLocalMode', '_handleLocalPipelineResult',
      '_runMlxSetup', '_stopCapture', 'stop', '_createNewSession',
      '_endSession', '_updateStartButton', '_updateEndButtonVisibility',
      '_updateControlsForMode', '_formatDuration', '_saveTranscriptFile',
      '_updateStatus',
    ],
  },
  'conversations.js': {
    obj: 'conversationMethods',
    desc: 'Sidebar session list, read-only transcript view, session meta',
    methods: [
      '_toggleSidebar', '_openConversationReadOnly',
      '_parseSavedTranscriptToSegments', '_loadConversationList',
      '_parseSessionMeta', '_formatBytes',
    ],
  },
  'window.js': {
    obj: 'windowMethods',
    desc: 'Window position persistence, pin/always-on-top, font size',
    methods: [
      '_saveWindowPosition', '_restoreWindowPosition', '_togglePin',
      '_adjustFontSize',
    ],
  },
  'updater-ui.js': {
    obj: 'updaterMethods',
    desc: 'Update check flow and the update-available toast',
    methods: ['_checkForUpdates', '_triggerUpdateCheck', '_onUpdateAvailable'],
  },
  'shortcuts.js': {
    obj: 'shortcutMethods',
    desc: 'Global keyboard shortcuts (Cmd/Ctrl+Enter start/stop etc.)',
    methods: ['_bindKeyboardShortcuts'],
  },
  'interview-panel.js': {
    obj: 'interviewPanelMethods',
    desc: 'Interview mode: suggestions panel, uploads/ingest, chat, streaming render',
    methods: [
      '_isSuggestionsMode', '_suggestionsPanelTitle', '_suggestionsEmptyText',
      '_updateSuggestionsPanelChrome', '_updateSuggestionsEmptyState',
      '_clearSuggestionsPanel', '_suggestionKindLabel', '_prependSuggestionKindLabel',
      '_setTemplateMode', '_dockInterviewSuggestionsRight', '_initRightPanelResizer',
      '_setRightPanelCollapsed', '_setMobileSheetOpen', '_undockInterviewSuggestions',
      '_isAllowedInterviewFile', '_updateInterviewUploadPills',
      '_initInterviewUploads', '_initTemplateDropdown', '_sendChatMessage',
      '_consumePickedSuggestion', '_getInterviewUserId', '_scheduleInterviewIngest',
      '_ingestInterviewFilesNow', '_onInterviewSpeakerFinal',
      '_injectBrainstormButton', '_scheduleSuggestions',
      '_setInterviewSuggestionsStatus', '_markInterviewSuggestStart',
      '_markInterviewSuggestDone', '_cancelInterviewSuggestionsStreaming',
      '_renderInterviewSuggestionsStream', '_runInterviewSuggestions',
      '_normalizeInterviewSuggestionItems', '_suggestionFaceText',
      '_suggestionChipLabel', '_renderInterviewSuggestions',
    ],
  },
};

// Methods that stay in app.js (core orchestration + shared utils)
const KEEP = new Set([
  'constructor', 'init', '_bindEvents', '_showView', '_showToast',
  '_insertIntoTextarea',
]);

// ── tiny lexical scanner: gives code-only mask (strings/templates/comments/regex blanked) ──
function codeMask(s) {
  const mask = new Array(s.length).fill(1); // 1 = code
  let i = 0, prevSig = '';
  const isRegexCtx = () =>
    prevSig === '' || '([{=,:;!&|?+-*%^~<>'.includes(prevSig);
  const WORDS = new Set(['return', 'typeof', 'case', 'in', 'of', 'new', 'delete', 'void', 'throw', 'else', 'do', 'yield', 'await']);
  const word = /[A-Za-z_$][\w$]*/y;

  while (i < s.length) {
    const c = s[i];
    const two = s.substr(i, 2);
    if (two === '//') {
      while (i < s.length && s[i] !== '\n') mask[i++] = 0;
      continue;
    }
    if (two === '/*') {
      mask[i] = 0; mask[i + 1] = 0; i += 2;
      while (i < s.length && s.substr(i, 2) !== '*/') mask[i++] = 0;
      if (i < s.length) { mask[i] = 0; mask[i + 1] = 0; i += 2; }
      continue;
    }
    if (c === "'" || c === '"' || c === '`') {
      const quote = c;
      mask[i++] = 0;
      let hitExpr = false;
      while (i < s.length) {
        if (s[i] === '\\') { mask[i] = 0; mask[i + 1] = 0; i += 2; continue; }
        // template: ${...} contains real code with braces — keep mask on inside
        if (quote === '`' && s.substr(i, 2) === '${') { i += 2; hitExpr = true; break; }
        if (s[i] === quote) { mask[i++] = 0; break; }
        mask[i++] = 0;
      }
      // For a template literal that hit ${, we drop back to code but need to
      // resume masking when the matching } closes the interpolation.
      if (quote === '`' && hitExpr) {
        // walk the ${...} region with a depth counter
        let depth = 1;
        let j = i;
        // NOTE: i already advanced past '${'; mark from j
        while (j < s.length && depth > 0) {
          const ch = s[j];
          if (ch === "'" || ch === '"') {
            const q = ch; j++;
            while (j < s.length && s[j] !== q) { if (s[j] === '\\') j++; j++; }
            j++; continue;
          }
          if (s.substr(j, 2) === '//') { while (j < s.length && s[j] !== '\n') j++; continue; }
          if (s.substr(j, 2) === '/*') { j += 2; while (j < s.length && s.substr(j, 2) !== '*/') j++; j += 2; continue; }
          if (ch === '{') depth++;
          else if (ch === '}') depth--;
          j++;
        }
        // after }, resume template masking until closing backtick
        while (j < s.length) {
          if (s[j] === '\\') { mask[j] = 0; mask[j + 1] = 0; j += 2; continue; }
          if (s[j] === '`') { mask[j] = 0; j++; break; }
          if (s.substr(j, 2) === '${') {
            // nested ${} — recurse by switching back (rare; treat chars as code for brace counting)
            let d2 = 1; j += 2;
            while (j < s.length && d2 > 0) {
              if (s[j] === '{') d2++; else if (s[j] === '}') d2--; j++;
            }
            continue;
          }
          mask[j++] = 0;
        }
        i = j;
      }
      prevSig = 'x';
      continue;
    }
    if (c === '/' && isRegexCtx()) {
      mask[i++] = 0;
      let inClass = false;
      while (i < s.length) {
        const ch = s[i];
        if (ch === '\\') { mask[i] = 0; mask[i + 1] = 0; i += 2; continue; }
        if (ch === '[') inClass = true;
        if (ch === ']') inClass = false;
        if (ch === '/' && !inClass) { mask[i++] = 0; break; }
        if (ch === '\n') break; // unterminated -> bail, treat rest as code
        mask[i++] = 0;
      }
      prevSig = 'x';
      continue;
    }
    if (!/\s/.test(c)) {
      if (/[\w$]/.test(c)) {
        word.lastIndex = i;
        const m = word.exec(s);
        prevSig = (m && WORDS.has(m[0])) ? '' : c;
      } else {
        prevSig = c;
      }
    }
    i++;
  }
  return mask;
}

const mask = codeMask(src);

// cumulative offset of each line start
const lineStart = [];
{
  let off = 0;
  for (const l of lines) { lineStart.push(off); off += l.length + 1; }
}

// locate `class App {` body span
const classMatch = src.match(/^class App\s*\{/m);
if (!classMatch) throw new Error('class App not found');
const classOpen = classMatch.index + classMatch[0].length - 1; // index of '{'
let depth = 0, classClose = -1;
for (let i = classOpen; i < src.length; i++) {
  if (!mask[i]) continue;
  if (src[i] === '{') depth++;
  else if (src[i] === '}') { depth--; if (depth === 0) { classClose = i; break; } }
}
if (classClose < 0) throw new Error('class close not found');

const bodyStartLine = lines.findIndex((_, i) => lineStart[i] > classOpen);
const bodyEndLine = lineStart.findIndex(v => v >= classClose); // line containing class '}' — usually the '}' is at line start
// simpler: find line index whose start <= classClose < start+len
const classCloseLine = lineStart.findIndex((v, i) => v <= classClose && classClose < v + lines[i].length + 1);

// ── find method spans (signature line at class depth 1) ────────────────────
const methodRe = /^    (?:async\s+)?([#\w$]+)\s*\(/;
const methods = [];
{
  let i = classOpen;
  // iterate lines inside class body
  for (let li = 0; li < lines.length; li++) {
    const start = lineStart[li];
    if (start <= classOpen) continue;
    if (start >= classClose) break;
    const m = lines[li].match(methodRe);
    if (!m) continue;
    // ensure signature '(' is in code state (not inside a comment — line comments
    // can't start a method anyway since regex anchored at col 4)
    const parenIdx = start + lines[li].indexOf('(');
    if (!mask[parenIdx]) continue;
    // params may contain destructuring ({...}) — match the param paren first,
    // then take the first code-state '{' after the closing ')'
    let j = parenIdx;
    let pd = 0;
    for (; j < src.length; j++) {
      if (!mask[j]) continue;
      if (src[j] === '(') pd++;
      else if (src[j] === ')') { pd--; if (pd === 0) { j++; break; } }
    }
    while (j < src.length && !(src[j] === '{' && mask[j])) j++;
    if (src[j] !== '{') throw new Error(`no body brace for ${m[1]}`);
    // match brace
    let d = 0, end = -1;
    for (let k = j; k < src.length; k++) {
      if (!mask[k]) continue;
      if (src[k] === '{') d++;
      else if (src[k] === '}') { d--; if (d === 0) { end = k; break; } }
    }
    if (end < 0) throw new Error(`unclosed method ${m[1]}`);
    const endLine = lineStart.findIndex((v, x) => v <= end && end < v + lines[x].length + 1);
    methods.push({ name: m[1], startLine: li, endLine });
    li = endLine; // skip nested lines
  }
}

// debug: print spans
for (const m of methods) console.log(`  ${m.name}: lines ${m.startLine + 1}-${m.endLine + 1}`);
console.log(`found ${methods.length} methods`);

// de-dup check
const names = methods.map(m => m.name);
const dup = names.filter((n, i) => names.indexOf(n) !== i);
if (dup.length) throw new Error('duplicate methods: ' + dup.join(','));

// coverage check: every method assigned to exactly one module or KEEP
const assigned = new Set();
for (const [file, mod] of Object.entries(MODULES)) {
  for (const n of mod.methods) {
    if (assigned.has(n)) throw new Error(`${n} assigned twice`);
    assigned.add(n);
  }
}
const unassigned = names.filter(n => !assigned.has(n) && !KEEP.has(n));
const missing = [...assigned].filter(n => !names.includes(n));
if (unassigned.length) throw new Error('methods with no module: ' + unassigned.join(', '));
if (missing.length) throw new Error('assigned but not in class: ' + missing.join(', '));

// ── build spans including preceding gap (comments/blank lines) ─────────────
const spans = methods.map((m, i) => {
  const prevEnd = i === 0 ? bodyStartLine - 1 : methods[i - 1].endLine;
  return { ...m, gapStart: prevEnd + 1 };
});

const toModule = new Map(); // Map, not {} — 'constructor' must not hit Object.prototype
for (const [file, mod] of Object.entries(MODULES)) {
  for (const n of mod.methods) toModule.set(n, file);
}

// emit module bodies
const moduleText = {}; // file -> array of line-chunks
const appKept = [];
for (const sp of spans) {
  const gap = lines.slice(sp.gapStart, sp.startLine);
  const body = lines.slice(sp.startLine, sp.endLine + 1);
  const chunk = [...gap, ...body];
  const file = toModule.get(sp.name);
  if (file) (moduleText[file] = moduleText[file] || []).push(chunk);
  else appKept.push(chunk);
}

// ── render module files ────────────────────────────────────────────────────
const DEP_RES = [
  ['invoke',        "const { invoke } = window.__TAURI__.core;"],
  ['getCurrentWindow', "const { getCurrentWindow } = window.__TAURI__.window;"],
  ['listen',        "const { listen } = window.__TAURI__.event;"],
  ['settingsManager', "import { settingsManager } from './settings.js';"],
  ['TranscriptUI',  "import { TranscriptUI } from './ui.js';"],
  ['sonioxClient',  "import { sonioxClient } from './soniox.js';"],
  ['elevenLabsTTS', "import { elevenLabsTTS } from './elevenlabs-tts.js';"],
  ['googleTTS',     "import { googleTTS } from './google-tts.js';"],
  ['edgeTTSRust',   "import { edgeTTSRust } from './edge-tts.js';"],
  ['audioPlayer',   "import { audioPlayer } from './audio-player.js';"],
  ['updater',       "import { updater } from './updater.js';"],
];

const written = [];
for (const [file, mod] of Object.entries(MODULES)) {
  const chunks = moduleText[file] || [];
  const text = chunks.map(c => c.join('\n')).join('\n');
  const tauri = DEP_RES.filter(([, line]) => line.startsWith('const'));
  const imps = DEP_RES.filter(([, line]) => line.startsWith('import'));
  const usedTauri = tauri.filter(([id]) => new RegExp(`\\b${id}\\b`).test(text));
  const usedImps = imps.filter(([id]) => new RegExp(`\\b${id}\\b`).test(text));

  const out = [
    `// ${mod.desc}`,
    `// Extracted from app.js — methods are merged onto App.prototype via Object.assign.`,
    ``,
    ...usedImps.map(([, l]) => l),
    ...(usedImps.length ? [''] : []),
    ...usedTauri.map(([, l]) => l),
    ``,
    `export const ${mod.obj} = {`,
    // Object-literal members need ',' between them; each chunk ends with the
    // method's closing '}' line.
    ...chunks.flatMap(c => {
      const cc = [...c];
      cc[cc.length - 1] = cc[cc.length - 1] + ',';
      return [...cc, ''];
    }),
    `};`,
    ``,
  ].join('\n');

  const p = path.join(JS_DIR, file);
  if (WRITE) fs.writeFileSync(p, out);
  written.push({ file, lines: out.split('\n').length, methods: chunks.length });
}

// ── rebuild app.js ─────────────────────────────────────────────────────────
const header = lines.slice(0, methods.length ? spans[0].gapStart - (spans[0].gapStart - 0) : 0);
// simpler: everything before the first line inside the class body minus class decl lines
const classStartLine = lineStart.findIndex((v, i) => v <= classOpen && classOpen < v + lines[i].length + 1);
const preClass = lines.slice(0, classStartLine + 1); // up to and incl 'class App {'
const postClass = lines.slice(classCloseLine + 1);   // after '}' line

const newImports = Object.entries(MODULES).map(
  ([file, mod]) => `import { ${mod.obj} } from './${file}';`
);

const appOut = [
  ...preClass.slice(0, -1),
  ...newImports,
  '',
  preClass[preClass.length - 1], // 'class App {'
  ...appKept.flatMap(c => [...c]),
  '}',
  '',
  '// Methods split into sibling modules are merged onto the prototype here.',
  `Object.assign(App.prototype, ${Object.values(MODULES).map(m => m.obj).join(', ')});`,
  ...postClass,
].join('\n');

const report = {
  methods: methods.length,
  kept: appKept.length,
  modules: written,
  appLines: appOut.split('\n').length,
};
console.log(JSON.stringify(report, null, 2));

if (WRITE) {
  // Safety net: keep a one-shot backup of the pre-split app.js.
  fs.writeFileSync(APP_PATH + '.bak', src);
  fs.writeFileSync(APP_PATH, appOut);
}
console.log(WRITE ? 'WROTE FILES (backup at app.js.bak)' : 'DRY RUN — pass --write to emit');
