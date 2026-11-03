import { describe, test, expect } from 'vitest';
import { minutesSections, reviewAsText } from '../src/js/review.js';

const minutes = {
    kind: 'minutes',
    overall: 'Agreed on the rollout.',
    decisions: ['Ship v2 on Monday'],
    action_items: [
        { task: 'Write rollout doc', owner: 'Me', due: 'Friday' },
        { task: 'Check budget', owner: '', due: '' },
    ],
    open_questions: [],
};

describe('minutesSections', () => {
    test('formats action items with owner and due date, and drops empty sections', () => {
        expect(minutesSections(minutes)).toEqual([
            { title: 'Decisions', items: ['Ship v2 on Monday'] },
            { title: 'Action items', items: ['Write rollout doc — Me (due Friday)', 'Check budget'] },
        ]);
    });
});

describe('reviewAsText', () => {
    test('meeting minutes copy as titled sections', () => {
        expect(reviewAsText(minutes)).toBe(
            'Overall: Agreed on the rollout.\n\nDecisions:\n- Ship v2 on Monday\n\n'
            + 'Action items:\n- Write rollout doc — Me (due Friday)\n- Check budget\n',
        );
    });

    test('interview reviews (no kind) keep the scored format', () => {
        const text = reviewAsText({
            overall: 'Good',
            questions: [{ question: 'Q1', answer_summary: 'A1', score: 4, feedback: 'F', stronger_answer: 'S' }],
            practice: ['P1'],
        });
        expect(text).toBe('Overall: Good\n\nQ: Q1\nA: A1\nScore: ●●●●○  F\nBetter: S\n\nPractice:\n- P1');
    });
});
