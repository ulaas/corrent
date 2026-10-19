import { describe, expect, it } from 'vitest';
import { chapterKind, skipAt } from '../src/lib/media';

// Same cases as corrent-proto's chapter_titles test: both clients must agree.
describe('skippable chapters', () => {
  it('classifies titles', () => {
    expect(chapterKind('Opening')).toBe('intro');
    expect(chapterKind('OP')).toBe('intro');
    expect(chapterKind('Opening Credits')).toBe('intro');
    expect(chapterKind('Previously on Show')).toBe('recap');
    expect(chapterKind('End Credits')).toBe('credits');
    expect(chapterKind('ED')).toBe('credits');
    expect(chapterKind('Chapter 3')).toBeNull();
  });
  it('finds the chapter and its end', () => {
    const ch = [
      { start_ms: 0, title: 'Cold open' },
      { start_ms: 60_000, title: 'Opening' },
      { start_ms: 150_000, title: 'Part A' },
    ];
    expect(skipAt(ch, 70, 1300)).toEqual({ kind: 'intro', end: 150 });
    expect(skipAt(ch, 149, 1300)).toBeNull();
    expect(skipAt(ch, 20, 1300)).toBeNull();
  });
});
