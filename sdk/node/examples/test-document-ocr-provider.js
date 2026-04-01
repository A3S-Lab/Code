const {
  Agent,
  DocumentParserRegistry,
} = require('../index.js');

async function main() {
  const agent = await Agent.create('agent.hcl');

  const session = agent.session('.', {
    documentParserRegistry: new DocumentParserRegistry({
      ocr: {
        enabled: true,
        model: 'openai/gpt-4.1-mini',
        maxImages: 2,
        dpi: 144,
      },
    }),
    documentOcrProvider: {
      name: 'node-mock-ocr',
      formats: ['pdf', 'image'],
      model: 'openai/gpt-4.1-mini',
      handler(request) {
        console.log('OCR request path:', request.path);
        console.log('OCR request format:', request.format);
        console.log('OCR request config:', request.config);

        if (request.format === 'pdf' || request.format === 'image') {
          return 'Recovered OCR text from Node backend.';
        }
        return null;
      },
    },
  });

  const tool = await session.tool('agentic_parse', { path: 'docs/scanned.pdf' });
  console.log('tool.output:', tool.output);
  console.log('tool.documentRuntime:', tool.documentRuntime);

  const runtime = tool.documentRuntime;
  if (runtime && runtime.ocr) {
    console.log('ocr.used:', runtime.ocr.used);
    console.log('ocr.provider:', runtime.ocr.provider);
    console.log('ocr.model:', runtime.ocr.model);
    console.log('ocr.format:', runtime.ocr.format);
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
