# Piper phonemizer notices

The packaged phonemizer is @diffusionstudio/piper-wasm 1.0.0 (MIT wrapper):
https://github.com/diffusionstudio/piper-wasm
Its README contains the build recipe and source repositories.

It includes Piper phonemize (MIT): https://github.com/rhasspy/piper-phonemize
and eSpeak NG (GPL-3.0-or-later): https://github.com/rhasspy/espeak-ng
The eSpeak NG license is included in COPYING-espeak-ng.

Danish voice: rhasspy/piper-voices, revision e9c8a39444359388adaa94255becb37a5a2b8e51,
da_DK-talesyntese-medium. The original MODEL_CARD is downloaded alongside the model
and preserved. Its training notes identify the Lessac base; the dataset CC0 label
alone does not describe the licensing of the full training chain.
Model source: https://huggingface.co/rhasspy/piper-voices/tree/e9c8a39444359388adaa94255becb37a5a2b8e51/da/da_DK/talesyntese/medium
