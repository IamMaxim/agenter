import { describe, expect, test } from 'vitest';
import { render } from 'svelte/server';
import PlanCard from './PlanCard.svelte';
import type { ChatItem } from '../lib/chatEvents';

const planItem: Extract<ChatItem, { kind: 'plan' }> = {
  id: 'plan:plan-1',
  kind: 'plan',
  title: 'Implementation plan',
  content: '1. Add markdown\n2. Restyle chat'
};

describe('PlanCard', () => {
  test('does not render the handoff row when no plan is pending', () => {
    const { body } = render(PlanCard, { props: { item: planItem } });
    expect(body).not.toContain('Implement plan');
    expect(body).not.toContain('Implement in fresh thread');
    expect(body).not.toContain('Stay in Plan mode');
  });

  test('renders three handoff actions when pendingHandoff is true', () => {
    const { body } = render(PlanCard, {
      props: {
        item: planItem,
        pendingHandoff: true,
        turnActive: false,
        defaultModeAvailable: true
      }
    });
    expect(body).toContain('Implement plan');
    expect(body).toContain('Implement in fresh thread');
    expect(body).toContain('Stay in Plan mode');
    expect(body).toContain('Implement this plan?');
    expect(body).not.toMatch(/Implement plan[^<]*<\/button>[\s\S]*disabled/);
  });

  test('disables both Implement actions when default mode is unavailable', () => {
    const { body } = render(PlanCard, {
      props: {
        item: planItem,
        pendingHandoff: true,
        turnActive: false,
        defaultModeAvailable: false
      }
    });
    const implementButton = extractButton(body, 'Implement plan');
    const freshThreadButton = extractButton(body, 'Implement in fresh thread');
    const stayButton = extractButton(body, 'Stay in Plan mode');
    expect(implementButton).toContain('disabled');
    expect(freshThreadButton).toContain('disabled');
    expect(stayButton).not.toContain('disabled');
  });

  test('disables every action while a turn is running', () => {
    const { body } = render(PlanCard, {
      props: {
        item: planItem,
        pendingHandoff: true,
        turnActive: true,
        defaultModeAvailable: true
      }
    });
    expect(extractButton(body, 'Implement plan')).toContain('disabled');
    expect(extractButton(body, 'Implement in fresh thread')).toContain('disabled');
    expect(extractButton(body, 'Stay in Plan mode')).toContain('disabled');
  });
});

function extractButton(body: string, label: string): string {
  const idx = body.indexOf(label);
  if (idx < 0) {
    throw new Error(`Button with label '${label}' not found in body`);
  }
  const start = body.lastIndexOf('<button', idx);
  const end = body.indexOf('</button>', idx);
  return body.slice(start, end + '</button>'.length);
}
