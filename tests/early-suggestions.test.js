import { describe, test, expect } from 'vitest';
import { sonioxClient, sonioxMicClient } from '../src/js/soniox.js';

describe('dual Soniox clients', () => {
    test('mic client is an independent SonioxClient instance', () => {
        expect(sonioxMicClient).not.toBe(sonioxClient);
        sonioxClient.isConnected = true;
        expect(sonioxMicClient.isConnected).toBe(false);
        sonioxClient.isConnected = false;
    });
});
