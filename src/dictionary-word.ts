export function dictionaryLookupWord(surface:string, draft:string, reused:string|undefined, verified:string|undefined) {
  const normalize = (word:string) => word.trim().toLowerCase();
  // An explicit edit can override the automatically verified headword.
  if (draft.trim() && normalize(draft)!==normalize(surface) &&
      (!verified || normalize(draft)!==normalize(verified))) return draft.trim();
  return verified || reused || draft.trim() || surface;
}
