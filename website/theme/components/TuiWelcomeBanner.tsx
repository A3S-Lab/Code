const tuiMascot = [
  '     .-^-.',
  '    /_____\\',
  '    ( o o )',
  '  |  /|_|\\  _',
  ' -+- |   | |#|',
  '  |  |___| \\#/',
  '     /   \\',
].join('\n');

const tuiWordmarkGlyphs = {
  A: ['01110', '10001', '10001', '11111', '10001', '10001', '10001'],
  '3': ['11110', '00001', '00001', '01110', '00001', '00001', '11110'],
  S: ['01111', '10000', '10000', '01110', '00001', '00001', '11110'],
  C: ['01111', '10000', '10000', '10000', '10000', '10000', '01111'],
  O: ['01110', '10001', '10001', '10001', '10001', '10001', '01110'],
  D: ['11110', '10001', '10001', '10001', '10001', '10001', '11110'],
  E: ['11111', '10000', '10000', '11110', '10000', '10000', '11111'],
} as const;

const tuiWordmarkVector = (() => {
  const commands: string[] = [];
  let offset = 0;

  for (const character of 'A3S CODE') {
    if (character === ' ') {
      offset += 3;
      continue;
    }

    const glyph =
      tuiWordmarkGlyphs[character as keyof typeof tuiWordmarkGlyphs];
    glyph.forEach((row, y) => {
      [...row].forEach((cell, x) => {
        if (cell === '1') commands.push(`M${offset + x} ${y}h1v1h-1z`);
      });
    });
    offset += 6;
  }

  return {
    path: commands.join(''),
    width: Math.max(offset - 1, 1),
  };
})();

function TuiWordmark() {
  return (
    <svg
      aria-hidden="true"
      className="a3s-tui-wordmark"
      focusable="false"
      preserveAspectRatio="xMinYMid meet"
      viewBox={`0 0 ${tuiWordmarkVector.width} 7`}
    >
      <path d={tuiWordmarkVector.path} />
    </svg>
  );
}

export function TuiWelcomeBanner({
  workspace = '~/workspace/a3s',
}: {
  workspace?: string;
}) {
  return (
    <>
      <div className="a3s-tui-welcome" aria-label="A3S Code">
        <pre aria-hidden="true" className="a3s-tui-mascot">
          {tuiMascot}
        </pre>
        <TuiWordmark />
      </div>
      <p className="a3s-tui-meta">
        <span>a3s-code v8.0.0</span>
        <i>·</i>
        <span>openai/gpt-5</span>
        <i>·</i>
        <span>12 skills</span>
        <i>·</i>
        <span>{workspace}</span>
      </p>
      <p className="a3s-tui-tip">
        Type a message · / for commands · Shift+Tab cycles mode · Ctrl+C twice
        to exit
      </p>
    </>
  );
}
