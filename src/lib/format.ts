// Small display formatters shared by the usage stats surfaces.

/** Token count → compact text: 999 → "999", 1234 → "1.2k", 1234567 → "1.2M". */
export function formatTokens(n: number): string {
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + 'M';
  if (n >= 1_000) return (n / 1_000).toFixed(1) + 'k';
  return String(n);
}
