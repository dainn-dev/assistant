import { describe, it, expect } from 'vitest';
import { conversationMethods } from '../src/js/conversations.js';

const parse = conversationMethods._parseSavedTranscriptToSegments;
const parseMeta = conversationMethods._parseSessionMeta;

describe('_parseSavedTranscriptToSegments', () => {
  it('returns [] for empty input', () => {
    expect(parse('')).toEqual([]);
    expect(parse('   \n  ')).toEqual([]);
  });

  it('strips frontmatter and pairs original + translation lines', () => {
    const md = [
      '---',
      'duration: 1m 0s',
      'segments: 1',
      '---',
      '',
      '**Speaker 1:**',
      '> Hello there',
      'Xin chào',
      '',
    ].join('\n');
    const segs = parse(md);
    expect(segs).toHaveLength(1);
    expect(segs[0].original).toBe('Hello there');
    expect(segs[0].translation).toBe('Xin chào');
    expect(segs[0].speaker).toBe('1'); // "Speaker 1" normalized to "1"
    expect(segs[0].status).toBe('translated');
  });

  it('flushes an unmatched original before the next one', () => {
    const md = '> First\n> Second\nDịch hai\n';
    const segs = parse(md);
    expect(segs).toHaveLength(2);
    expect(segs[0].original).toBe('First');
    expect(segs[0].translation).toBe('');
    expect(segs[1].original).toBe('Second');
    expect(segs[1].translation).toBe('Dịch hai');
  });

  it('keeps speaker attribution across segments', () => {
    const md = '**Speaker 2:**\n> Hi\nChào\n\n**Speaker 1:**\n> Yo\nNày\n';
    const segs = parse(md);
    expect(segs[0].speaker).toBe('2');
    expect(segs[1].speaker).toBe('1');
  });
});

describe('_parseSessionMeta', () => {
  it('splits created_at into date and HH:MM time', () => {
    const meta = parseMeta({ created_at: '2026-03-27 10:21:05' });
    expect(meta.date).toBe('2026-03-27');
    expect(meta.time).toBe('10:21');
  });

  it('handles missing created_at', () => {
    const meta = parseMeta({});
    expect(meta.date).toBe('');
    expect(meta.time).toBe('');
  });
});
