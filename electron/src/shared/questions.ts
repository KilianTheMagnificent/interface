/**
 * Structured questions a provider can park on, the way Claude's AskUserQuestion
 * tool does. Parsed from the approval's `input` so any engine that ships the
 * same shape gets the same picker without a per-vendor event kind.
 */

export interface AgentQuestionOption {
  label: string;
  description: string;
  preview?: string;
}

export interface AgentQuestion {
  question: string;
  header: string;
  options: AgentQuestionOption[];
  multiSelect: boolean;
}

const QUESTION_TOOLS = /askuserquestion|ask_user_question|askquestion/i;

export function isQuestionTool(tool: string): boolean {
  return QUESTION_TOOLS.test(tool);
}

export function parseAgentQuestions(tool: string, input: unknown):
    AgentQuestion[] | undefined {
  const rec = asRecord(input);
  const raw = rec === undefined ? undefined : rec.questions;
  if (!Array.isArray(raw) || raw.length === 0) {
    return isQuestionTool(tool) ? [] : undefined;
  }
  const out: AgentQuestion[] = [];
  for (const entry of raw) {
    const parsed = parseQuestion(entry);
    if (parsed !== undefined) out.push(parsed);
  }
  return out.length > 0 ? out : (isQuestionTool(tool) ? [] : undefined);
}

export function questionSummary(questions: readonly AgentQuestion[]): string {
  if (questions.length === 0) return "The agent has a question.";
  if (questions.length === 1) return questions[0].question;
  return `${questions.length} questions: ${questions[0].question}`;
}

function parseQuestion(raw: unknown): AgentQuestion | undefined {
  const rec = asRecord(raw);
  if (rec === undefined) return undefined;
  const question = asText(rec.question);
  if (question === undefined) return undefined;
  const options: AgentQuestionOption[] = [];
  if (Array.isArray(rec.options)) {
    for (const option of rec.options) {
      const row = asRecord(option);
      const label = row === undefined ? undefined : asText(row.label);
      if (label === undefined) continue;
      const description = row === undefined ? undefined : asText(row.description);
      const preview = row === undefined ? undefined : asText(row.preview);
      options.push({
        label,
        description: description ?? "",
        ...(preview === undefined ? {} : { preview }),
      });
    }
  }
  if (options.length < 2) return undefined;
  return {
    question,
    header: asText(rec.header) ?? question.slice(0, 12),
    options,
    multiSelect: rec.multiSelect === true,
  };
}

function asRecord(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown> : undefined;
}

function asText(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}
