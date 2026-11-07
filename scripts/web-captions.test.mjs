import assert from 'node:assert/strict';
import test from 'node:test';
import {captionNodes} from '../dist/web-captions.js';

const image = {type:'image',index:0,alt:''};
const element = (tag,...children) => ({type:'element',tag,children});
const text = (start,end) => ({type:'text',start,end});

test('sizes explicit captions and short unmarked paragraphs directly after an image', () => {
  const description = 'Mud tablet from the collection.';
  const body = 'This is ordinary article text. '.repeat(8);
  const caption = element('p',text(0,description.length));
  const paragraph = element('p',text(description.length,description.length+body.length));
  const explicit = element('figcaption',text(0,description.length));
  const nodes = [element('p',image),caption,paragraph,element('figure',image,explicit)];
  const found = captionNodes(nodes,description+body);
  assert.ok(found.has(caption));
  assert.ok(found.has(explicit));
  assert.ok(!found.has(paragraph));
});

test('does not resize the paragraph after a figure that already has a caption', () => {
  const explicit = element('figcaption',text(0,9));
  const paragraph = element('p',text(9,20));
  const found = captionNodes([element('figure',image,explicit),paragraph],'A captionA short intro');
  assert.ok(found.has(explicit));
  assert.ok(!found.has(paragraph));
});

test('recognizes a long caption in a dedicated image-and-paragraph wrapper', () => {
  const description = 'A long caption with an italicized source attribution. '.repeat(7);
  const body = 'The article continues here.';
  const caption = element('p',text(0,20),element('i',text(20,description.length)));
  const paragraph = element('p',text(description.length,description.length+body.length));
  const found = captionNodes([element('div',image,caption),paragraph],description+body);
  assert.ok(found.has(caption));
  assert.ok(!found.has(paragraph));
});
