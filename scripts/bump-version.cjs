#!/usr/bin/env node
// Bump the app version in all three places that define it:
//   package.json, src-tauri/tauri.conf.json, src-tauri/Cargo.toml
// then sync Cargo.lock via `cargo update -p myjavis`.
//
// Usage: npm run bump 1.1.0

const fs = require('fs');
const path = require('path');
const { execFileSync } = require('child_process');

const ROOT = path.resolve(__dirname, '..');
const VERSION_RE = /^\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$/;

/**
 * @param {{pkg: string, conf: string, cargo: string}} files
 * @param {string} v new version
 * @returns {{pkg: string, conf: string, cargo: string}}
 */
function applyVersion(files, v) {
    if (!VERSION_RE.test(v)) {
        throw new Error(`Invalid version "${v}" — expected X.Y.Z or X.Y.Z-pre.N`);
    }
    const pkg = files.pkg.replace(
        /("version"\s*:\s*")[^"]*(")/,
        `$1${v}$2`
    );
    const conf = files.conf.replace(
        /("version"\s*:\s*")[^"]*(")/,
        `$1${v}$2`
    );
    const cargo = files.cargo.replace(
        /^version = "[^"]*"/m,
        `version = "${v}"`
    );
    if (pkg === files.pkg && !files.pkg.includes(v)) throw new Error('package.json: version field not found');
    if (conf === files.conf && !files.conf.includes(v)) throw new Error('tauri.conf.json: version field not found');
    if (cargo === files.cargo && !files.cargo.includes(v)) throw new Error('Cargo.toml: version field not found');
    return { pkg, conf, cargo };
}

function main() {
    const v = process.argv[2];
    if (!v) {
        console.error('Usage: npm run bump <version>');
        process.exit(1);
    }
    const paths = {
        pkg: path.join(ROOT, 'package.json'),
        conf: path.join(ROOT, 'src-tauri', 'tauri.conf.json'),
        cargo: path.join(ROOT, 'src-tauri', 'Cargo.toml'),
    };
    const files = Object.fromEntries(
        Object.entries(paths).map(([k, p]) => [k, fs.readFileSync(p, 'utf8')])
    );
    const out = applyVersion(files, v);
    for (const [k, p] of Object.entries(paths)) fs.writeFileSync(p, out[k]);

    execFileSync('cargo', ['update', '-p', 'myjavis', '--manifest-path', paths.cargo.replace(/Cargo\.toml$/, 'Cargo.toml')], {
        stdio: 'inherit',
        cwd: path.join(ROOT, 'src-tauri'),
    });
    console.log(`Bumped to v${v} — package.json, tauri.conf.json, Cargo.toml, Cargo.lock`);
}

if (require.main === module) main();

module.exports = { applyVersion };
