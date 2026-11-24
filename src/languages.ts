export const languages = [
  {code:'en',name:'英语',native:'English',locale:'en-US'},
  {code:'fr',name:'法语',native:'Français',locale:'fr-FR'},
  {code:'de',name:'德语',native:'Deutsch',locale:'de-DE'},
  {code:'es',name:'西班牙语',native:'Español',locale:'es-ES'},
  {code:'it',name:'意大利语',native:'Italiano',locale:'it-IT'},
  {code:'pt',name:'葡萄牙语',native:'Português',locale:'pt-PT'},
  {code:'ja',name:'日语',native:'日本語',locale:'ja-JP'},
  {code:'ko',name:'韩语',native:'한국어',locale:'ko-KR'},
  {code:'zh',name:'中文',native:'中文',locale:'zh-CN'},
  {code:'ru',name:'俄语',native:'Русский',locale:'ru-RU'},
  {code:'ar',name:'阿拉伯语',native:'العربية',locale:'ar-SA'},
  {code:'da',name:'丹麦语',native:'Dansk',locale:'da-DK'},
] as const;
function preference(key:string) {
  try { return typeof window==='undefined' ? null : window.localStorage.getItem(key); } catch { return null; }
}
export function currentLanguage() {
  const code=preference('learningLanguage');
  return languages.find(language=>language.code===code) ?? languages[0];
}
export function explanationLanguage() {
  return languages.find(language=>language.code===preference('explanationLanguage')) ?? languages.find(language=>language.code==='zh')!;
}
export function languageKey(key:string) {
  const code=currentLanguage().code;
  return code==='en' ? key : `language:${code}:${key}`;
}
