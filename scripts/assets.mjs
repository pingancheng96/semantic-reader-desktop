import {copyFile} from 'node:fs/promises';
for (const [from,to] of [
  ['index.html','index.html'], ['src/style.css','style.css'], ['src-tauri/icons/icon.png','icon.png'],
  ['node_modules/react/umd/react.production.min.js','react.js'],
  ['node_modules/react-dom/umd/react-dom.production.min.js','react-dom.js']
]) await copyFile(from, 'dist/' + to);
