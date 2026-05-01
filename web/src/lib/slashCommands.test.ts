import { describe, expect, test } from 'vitest';

import type { SlashCommandDefinition } from '../api/types';
import {
  filterSlashCommands,
  isSlashDraft,
  needsSlashConfirmation,
  parseSlashCommand,
  slashRequest
} from './slashCommands';

const commands: SlashCommandDefinition[] = [
  {
    id: 'codex.shell',
    name: 'shell',
    aliases: ['sh'],
    description: 'Run shell',
    category: 'provider',
    provider_id: 'codex',
    target: 'provider',
    danger_level: 'dangerous',
    arguments: [{ name: 'command', kind: 'rest', required: true, description: null, choices: [] }],
    examples: []
  },
  {
    id: 'codex.review',
    name: 'review',
    aliases: [],
    description: 'Review changes',
    category: 'provider',
    provider_id: 'codex',
    target: 'provider',
    danger_level: 'safe',
    arguments: [{ name: 'target', kind: 'rest', required: false, description: null, choices: [] }],
    examples: []
  },
  {
    id: 'codex.rollback',
    name: 'rollback',
    aliases: [],
    description: 'Rollback turns',
    category: 'provider',
    provider_id: 'codex',
    target: 'provider',
    danger_level: 'dangerous',
    arguments: [{ name: 'numTurns', kind: 'number', required: true, description: null, choices: [] }],
    examples: []
  }
];

describe('slash commands', () => {
  test('detects only leading slash commands', () => {
    expect(isSlashDraft('/review')).toBe(true);
    expect(isSlashDraft('please /review')).toBe(false);
    expect(isSlashDraft('//not-command')).toBe(false);
  });

  test('filters commands by name and alias', () => {
    expect(filterSlashCommands('/sh', commands).map((command) => command.id)).toEqual([
      'codex.shell'
    ]);
    expect(filterSlashCommands('/r', commands).map((command) => command.name)).toEqual([
      'review',
      'rollback'
    ]);
  });

  test('parses rest, numeric, and flag arguments', () => {
    expect(parseSlashCommand('/shell pwd | cat', commands).arguments).toEqual({
      command: 'pwd | cat'
    });
    expect(parseSlashCommand('/rollback 2', commands).arguments).toEqual({
      numTurns: 2
    });
    expect(parseSlashCommand('/review --base main --detached', commands).arguments).toEqual({
      base: 'main',
      detached: true
    });
  });

  test('builds execution requests and marks dangerous commands', () => {
    const shell = commands[0];
    expect(needsSlashConfirmation(shell)).toBe(true);
    expect(slashRequest('/shell pwd', shell, true)).toEqual({
      command_id: 'codex.shell',
      arguments: { command: 'pwd' },
      raw_input: '/shell pwd',
      confirmed: true
    });
  });
});
