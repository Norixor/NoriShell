export const MIN_NEW_SECRET_PASSWORD_CHARACTERS = 8;

export function passwordCharacterCount(password: string): number {
  return Array.from(password).length;
}
