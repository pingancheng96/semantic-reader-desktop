export function waitForVoice(speech: SpeechSynthesis, locale: string, signal: AbortSignal, timeout = 2500) {
  return new Promise<SpeechSynthesisVoice | undefined>(resolve => {
    const language = locale.toLowerCase();
    function finish(voice?: SpeechSynthesisVoice) {
      clearTimeout(deadline);
      clearInterval(poll);
      speech.removeEventListener('voiceschanged', check);
      signal.removeEventListener('abort', abort);
      resolve(voice);
    }
    function check() {
      const local=speech.getVoices().filter(candidate=>candidate.localService);
      const voice = local.find(candidate=>candidate.lang.replace('_','-').toLowerCase()===language)
        ?? (language.startsWith('en-') ? undefined : local.find(candidate=>candidate.lang.replace('_','-').toLowerCase().split('-')[0]===language.split('-')[0]));
      if (voice) finish(voice);
    }
    function abort() { finish(); }
    const deadline = setTimeout(abort, timeout);
    const poll = setInterval(check, 100);
    speech.addEventListener('voiceschanged', check);
    signal.addEventListener('abort', abort, {once:true});
    if (signal.aborted) abort();
    else check();
  });
}
