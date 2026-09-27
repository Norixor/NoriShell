export interface TerminalOutputGeometryMarker {
  afterOutputSeq: string;
  rows: number;
  cols: number;
}

export function geometryForOutput(
  markers: readonly TerminalOutputGeometryMarker[],
  outputSeq: string,
): TerminalOutputGeometryMarker | null {
  const sequence = BigInt(outputSeq);
  let result: TerminalOutputGeometryMarker | null = null;
  for (const marker of markers) {
    if (BigInt(marker.afterOutputSeq) >= sequence) break;
    result = marker;
  }
  return result;
}

export function recordOutputGeometry(
  markers: TerminalOutputGeometryMarker[],
  afterOutputSeq: string,
  rows: number,
  cols: number,
): void {
  const previous = markers.at(-1);
  if (previous?.afterOutputSeq === afterOutputSeq) {
    previous.rows = rows;
    previous.cols = cols;
  } else if (previous?.rows !== rows || previous.cols !== cols) {
    markers.push({ afterOutputSeq, rows, cols });
  }
}
