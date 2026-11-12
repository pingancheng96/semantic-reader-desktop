import {copyFile, cp, mkdir} from 'node:fs/promises';
for (const [from,to] of [
  ['index.html','index.html'], ['src/style.css','style.css'], ['src-tauri/icons/icon.png','icon.png'],
  ['node_modules/react/umd/react.production.min.js','react.js'],
  ['node_modules/react-dom/umd/react-dom.production.min.js','react-dom.js']
]) await copyFile(from, 'dist/' + to);

await mkdir('dist/pdfjs', {recursive:true});
await mkdir('dist/ocr', {recursive:true});
for (const name of ['pdf.mjs','pdf.worker.mjs']) {
  await copyFile('node_modules/pdfjs-dist/legacy/build/'+name,'dist/pdfjs/'+name);
}
for (const name of ['cmaps','standard_fonts','wasm','iccs']) {
  await cp('node_modules/pdfjs-dist/'+name,'dist/pdfjs/'+name,{recursive:true});
}
for (const name of ['tesseract.min.js','worker.min.js']) {
  await copyFile('node_modules/tesseract.js/dist/'+name,'dist/ocr/'+name);
}
await copyFile('node_modules/tesseract.js-core/tesseract-core-lstm.wasm.js','dist/ocr/tesseract-core-lstm.wasm.js');
await copyFile('node_modules/@tesseract.js-data/eng/4.0.0_best_int/eng.traineddata.gz','dist/ocr/eng.traineddata.gz');

await mkdir('dist/ort', {recursive:true});
for (const name of ['ort.wasm.min.mjs','ort-wasm-simd-threaded.mjs','ort-wasm-simd-threaded.wasm']) {
  await copyFile('node_modules/onnxruntime-web/dist/'+name,'dist/ort/'+name);
}
