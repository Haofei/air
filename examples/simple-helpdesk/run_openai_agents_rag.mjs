import fs from 'node:fs';
import OpenAI from 'openai';
import {
  Agent,
  run,
  setDefaultOpenAIClient,
  setOpenAIAPI,
  setTracingDisabled,
  tool,
} from '@openai/agents';
import { z } from 'zod';

function readJson(path) {
  return JSON.parse(fs.readFileSync(path, 'utf8'));
}

function parseArgs(argv) {
  const args = {
    input: 'examples/simple-helpdesk/input.json',
    modelConfig: 'examples/bigmodel-openai-compatible.json',
    toolConfig: 'examples/simple-helpdesk/tools.json',
    modelAlias: 'rag_answerer',
  };

  for (let index = 2; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === '--input') args.input = argv[++index];
    else if (arg === '--model-config') args.modelConfig = argv[++index];
    else if (arg === '--tool-config') args.toolConfig = argv[++index];
    else if (arg === '--model-alias') args.modelAlias = argv[++index];
    else throw new Error(`unknown argument ${arg}`);
  }

  return args;
}

function configureOpenAICompatible(modelConfig, alias) {
  const model = modelConfig.models?.[alias];
  if (!model) throw new Error(`unknown model alias ${alias}`);

  const apiKey = process.env[model.api_key_env];
  if (!apiKey) throw new Error(`environment variable ${model.api_key_env} is not set`);

  setTracingDisabled(true);
  setOpenAIAPI('chat_completions');
  setDefaultOpenAIClient(
    new OpenAI({
      apiKey,
      baseURL: model.base_url,
    }),
  );

  return model;
}

function makeDocsSearchTool(toolConfig) {
  const configured = toolConfig.tools?.['docs.search'];
  if (!configured || configured.kind !== 'local_docs_search') {
    throw new Error('tool config must define tools["docs.search"] as local_docs_search');
  }

  const documents = configured.documents ?? [];
  return tool({
    name: 'docs_search',
    description:
      'Search the local helpdesk knowledge base. Use this before answering account support questions.',
    parameters: z.object({
      query: z.string(),
    }),
    async execute({ query }) {
      const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
      const matches = documents
        .filter((doc) =>
          terms.some(
            (term) =>
              doc.title.toLowerCase().includes(term) ||
              doc.content.toLowerCase().includes(term),
          ),
        )
        .slice(0, 3);
      console.log(`[openai-agents] docs_search -> ${matches.map((doc) => doc.id).join(',')}`);
      return {
        query,
        documents: matches,
      };
    },
  });
}

function parseJsonish(value) {
  if (typeof value !== 'string') return value;
  const trimmed = value.trim();
  try {
    return JSON.parse(trimmed);
  } catch {
    // Continue below.
  }

  if (trimmed.startsWith('```')) {
    const lines = trimmed.split(/\r?\n/);
    if (lines[0].startsWith('```')) lines.shift();
    if (lines.at(-1)?.trim() === '```') lines.pop();
    try {
      return JSON.parse(lines.join('\n').trim());
    } catch {
      return { content: value };
    }
  }

  return { content: value };
}

async function main() {
  const args = parseArgs(process.argv);
  const input = readJson(args.input);
  const modelConfig = readJson(args.modelConfig);
  const toolConfig = readJson(args.toolConfig);
  const model = configureOpenAICompatible(modelConfig, args.modelAlias);

  const agent = new Agent({
    name: 'Helpdesk RAG Agent',
    model: model.model,
    instructions:
      `${model.system_prompt ?? ''}\n\n` +
      'You must call docs_search once before answering. Use only returned documents. ' +
      'Return only a JSON object with keys answer, citations, escalation_required.',
    tools: [makeDocsSearchTool(toolConfig)],
    modelSettings: {
      temperature: model.temperature ?? 0,
      toolChoice: 'auto',
    },
  });

  const result = await run(
    agent,
    `Question: ${input.question}`,
    {
      maxTurns: 4,
    },
  );

  console.log(JSON.stringify(parseJsonish(result.finalOutput), null, 2));
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
