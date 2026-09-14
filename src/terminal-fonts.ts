export const TERMINAL_FONT_CANDIDATES = [
  "SF Mono",
  "Menlo",
  "Monaco",
  "JetBrains Mono",
  "Fira Code",
  "Cascadia Mono",
  "Cascadia Code",
  "Source Code Pro",
  "IBM Plex Mono",
  "Iosevka",
  "Hack",
  "Inconsolata",
  "Maple Mono",
  "Sarasa Mono SC",
  "Noto Sans Mono",
  "Ubuntu Mono",
  "Consolas",
  "Liberation Mono",
  "DejaVu Sans Mono",
] as const;

type LocalFontProbe = (fontFamily: string) => Promise<boolean>;

async function probeLocalFont(fontFamily: string) {
  try {
    const face = new FontFace(
      `nvx-terminal-font-${TERMINAL_FONT_CANDIDATES.indexOf(
        fontFamily as (typeof TERMINAL_FONT_CANDIDATES)[number],
      )}`,
      `local("${fontFamily}")`,
    );
    await face.load();
    return face.status === "loaded";
  } catch {
    return false;
  }
}

export async function detectInstalledTerminalFonts(
  probe: LocalFontProbe = probeLocalFont,
): Promise<string[]> {
  const availability = await Promise.all(
    TERMINAL_FONT_CANDIDATES.map(async (fontFamily) => ({
      fontFamily,
      available: await probe(fontFamily),
    })),
  );
  return availability
    .filter(({ available }) => available)
    .map(({ fontFamily }) => fontFamily);
}
