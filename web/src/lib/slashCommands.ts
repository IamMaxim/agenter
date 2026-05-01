import type {
  SlashCommandDefinition,
  SlashCommandRequest,
  SlashCommandResult
} from '../api/types';

export interface ParsedSlashCommand {
  command?: SlashCommandDefinition;
  query: string;
  arguments: Record<string, unknown>;
  missingRequired: string[];
  isSlash: boolean;
  error?: string;
}

export function isSlashDraft(draft: string): boolean {
  return draft.startsWith('/') && !draft.startsWith('//');
}

export function filterSlashCommands(
  draft: string,
  commands: SlashCommandDefinition[]
): SlashCommandDefinition[] {
  if (!isSlashDraft(draft)) {
    return [];
  }
  const query = commandToken(draft).toLowerCase();
  return commands
    .filter((command) =>
      [command.name, ...command.aliases].some((candidate) =>
        candidate.toLowerCase().startsWith(query)
      )
    )
    .sort((left, right) => left.name.localeCompare(right.name));
}

export function parseSlashCommand(
  draft: string,
  commands: SlashCommandDefinition[]
): ParsedSlashCommand {
  if (!isSlashDraft(draft)) {
    return { query: '', arguments: {}, missingRequired: [], isSlash: false };
  }
  const token = commandToken(draft);
  const command = commands.find((candidate) =>
    [candidate.name, ...candidate.aliases].includes(token)
  );
  if (!command) {
    return {
      query: token,
      arguments: {},
      missingRequired: [],
      isSlash: true,
      error: token ? `Unknown slash command /${token}.` : undefined
    };
  }
  const rest = draft.slice(token.length + 1).trim();
  const args = parseArguments(rest, command);
  const missingRequired = command.arguments
    .filter((argument) => argument.required && isMissing(args[argument.name]))
    .map((argument) => argument.name);
  return {
    command,
    query: token,
    arguments: args,
    missingRequired,
    isSlash: true
  };
}

export function slashRequest(
  draft: string,
  command: SlashCommandDefinition,
  confirmed: boolean
): SlashCommandRequest {
  const parsed = parseSlashCommand(draft, [command]);
  return {
    command_id: command.id,
    arguments: parsed.arguments,
    raw_input: draft.trim(),
    confirmed
  };
}

export function needsSlashConfirmation(command: SlashCommandDefinition): boolean {
  return command.danger_level === 'confirm' || command.danger_level === 'dangerous';
}

export function slashResultMessage(result: SlashCommandResult): string {
  return result.message || (result.accepted ? 'Command accepted.' : 'Command rejected.');
}

function commandToken(draft: string): string {
  const token = draft.slice(1).trimStart().split(/\s+/, 1)[0] ?? '';
  return token.toLowerCase();
}

function parseArguments(rest: string, command: SlashCommandDefinition): Record<string, unknown> {
  const args: Record<string, unknown> = {};
  const tokens = tokenize(rest);
  let cursor = 0;
  const { flags, consumed } = parseFlags(tokens);
  for (const argument of command.arguments) {
    const flagged = flags[argument.name];
    if (typeof flagged === 'string' || typeof flagged === 'boolean') {
      args[argument.name] = coerceArgument(flagged, argument.kind);
      continue;
    }
    if (argument.kind === 'rest') {
      const value = tokens
        .slice(cursor)
        .filter((_, index) => !consumed.has(index + cursor))
        .join(' ')
        .trim();
      if (value) {
        args[argument.name] = value;
      }
      cursor = tokens.length;
      continue;
    }
    const token = tokens[cursor];
    if (token !== undefined && !token.startsWith('--')) {
      args[argument.name] = coerceArgument(token, argument.kind);
      cursor += 1;
    }
  }
  for (const [key, value] of Object.entries(flags)) {
    if (args[key] === undefined) {
      args[key] = value;
    }
  }
  return args;
}

function parseFlags(tokens: string[]): { flags: Record<string, unknown>; consumed: Set<number> } {
  const flags: Record<string, unknown> = {};
  const consumed = new Set<number>();
  for (let index = 0; index < tokens.length; index += 1) {
    const token = tokens[index];
    if (!token.startsWith('--')) {
      continue;
    }
    consumed.add(index);
    const [rawName, inlineValue] = token.slice(2).split('=', 2);
    const name = flagName(rawName);
    if (inlineValue !== undefined) {
      flags[name] = inlineValue;
      continue;
    }
    const next = tokens[index + 1];
    if (next && !next.startsWith('--')) {
      flags[name] = next;
      consumed.add(index + 1);
      index += 1;
    } else {
      flags[name] = true;
    }
  }
  return { flags, consumed };
}

function flagName(name: string): string {
  if (name === 'base') {
    return 'base';
  }
  return name.replace(/-([a-z])/g, (_, char: string) => char.toUpperCase());
}

function tokenize(value: string): string[] {
  const matches = value.match(/"([^"]*)"|'([^']*)'|[^\s]+/g) ?? [];
  return matches.map((token) => {
    if (
      (token.startsWith('"') && token.endsWith('"')) ||
      (token.startsWith("'") && token.endsWith("'"))
    ) {
      return token.slice(1, -1);
    }
    return token;
  });
}

function coerceArgument(value: string | boolean, kind: SlashCommandDefinition['arguments'][number]['kind']): unknown {
  if (kind === 'number') {
    const number = typeof value === 'string' ? Number(value) : Number.NaN;
    return Number.isFinite(number) ? number : value;
  }
  return value;
}

function isMissing(value: unknown): boolean {
  return value === undefined || value === null || value === '';
}
