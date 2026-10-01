import { describe, it, expect } from 'vitest';
import { applyVersion } from '../scripts/bump-version.cjs';

const pkg = `{\n  "name": "myjavis",\n  "version": "1.0.0",\n  "private": true\n}\n`;
const conf = `{\n  "productName": "MyJavis",\n  "version": "1.0.0",\n  "identifier": "com.myjavis.app"\n}\n`;
const cargo = `[package]\nname = "myjavis"\nversion = "1.0.0"\ndescription = ""\n`;

describe('applyVersion', () => {
    it('rewrites exactly the three version fields', () => {
        const out = applyVersion({ pkg, conf, cargo }, '1.1.0');
        expect(out.pkg).toBe(`{\n  "name": "myjavis",\n  "version": "1.1.0",\n  "private": true\n}\n`);
        expect(out.conf).toBe(`{\n  "productName": "MyJavis",\n  "version": "1.1.0",\n  "identifier": "com.myjavis.app"\n}\n`);
        expect(out.cargo).toBe(`[package]\nname = "myjavis"\nversion = "1.1.0"\ndescription = ""\n`);
    });

    it('accepts prerelease tags like 1.0.1-rc.1', () => {
        const out = applyVersion({ pkg, conf, cargo }, '1.0.1-rc.1');
        expect(out.pkg).toContain('"version": "1.0.1-rc.1"');
        expect(out.cargo).toContain('version = "1.0.1-rc.1"');
    });

    it('rejects malformed versions', () => {
        expect(() => applyVersion({ pkg, conf, cargo }, '1.0')).toThrow(/version/i);
        expect(() => applyVersion({ pkg, conf, cargo }, 'v1.0.0')).toThrow(/version/i);
        expect(() => applyVersion({ pkg, conf, cargo }, '../../etc')).toThrow(/version/i);
    });
});
