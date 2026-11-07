import test from 'node:test';
import assert from 'node:assert/strict';
import {facets} from '../dist/tag-filters.js';

test('tag filters intersect in any order and only expose compatible tags', () => {
  const groups = ['a','b','c','d'].map(id=>({id}));
  const tags = ['history','writing','culture'].map(id=>({id,name:id,aliases:[],count:0}));
  const links = [
    ['a','history'], ['a','writing'], ['b','history'], ['b','culture'], ['c','writing']
  ].map(([group_id,tag_id])=>({group_id,tag_id}));
  const first = facets(groups,tags,links,['history'],false);
  assert.deepEqual(first.results.map(g=>g.id),['a','b']);
  assert.deepEqual(first.options.map(t=>[t.id,t.count]),[['writing',1],['culture',1]]);
  assert.deepEqual(facets(groups,tags,links,['history','writing'],false).results,[{id:'a'}]);
  assert.deepEqual(facets(groups,tags,links,['writing','history'],false).results,[{id:'a'}]);
  assert.deepEqual(facets(groups,tags,links,[],true).results,[{id:'d'}]);
});
