// Run with: node --test crates/graphicle/tests/test_graph_view.cjs
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const context = vm.createContext({});
vm.runInContext(fs.readFileSync(path.join(__dirname, '../web/workspace.js'), 'utf8'), context);
const focus = (data, id, relation) => context.graphFocus(data, id, relation);
const nodes = [
  {id: 'handler', name: 'Handle', source_file: 'api/handler.go'},
  {id: 'helper', name: 'Check', source_file: 'auth/check.go'},
  {id: 'duplicate', name: 'Check', source_file: 'policy/check.go'},
];
const edges = [
  {source: 'handler', target: 'helper', relation: 'calls', confidence: 'EXTRACTED'},
  {source: 'helper', target: 'duplicate', relation: 'may_call', confidence: 'INFERRED'},
];

test('focus preserves actual direction, duplicate names and uncertainty', () => {
  const view = focus({nodes, edges}, 'helper');
  assert.equal(view.selected.id, 'helper');
  assert.equal(view.incoming.length, 1);
  assert.equal(view.incoming[0].source, 'handler');
  assert.equal(view.outgoing.length, 1);
  assert.equal(view.outgoing[0].target, 'duplicate');
  assert.equal(view.outgoing[0].confidence, 'INFERRED');
  assert.equal(view.byId.size, 3);
});

test('relation filtering never reclassifies may_call as calls', () => {
  const view = focus({nodes, edges}, 'helper', 'calls');
  assert.equal(view.incoming.length, 1);
  assert.equal(view.outgoing.length, 0);
  assert.equal(view.edges.length, 2); // Full slice remains available to other filters.
});

test('cycles and recursive calls remain visible without duplicating self edges', () => {
  const cycle = {source: 'helper', target: 'handler', relation: 'calls'};
  const self = {source: 'helper', target: 'helper', relation: 'calls'};
  const view = focus({nodes, edges: [...edges, cycle, self]}, 'helper');
  assert.equal(view.incoming.length, 1);
  assert.equal(view.outgoing.length, 3);
  assert.equal(view.outgoing.filter(edge => edge === self).length, 1);
});

test('partial slices and drawing caps stay explicit', () => {
  assert.equal(focus({nodes, edges, truncated: true}).partial, true);
  assert.equal(focus({nodes, edges, edges_truncated: true}).partial, true);
  const view = focus({nodes, edges: Array(121).fill(edges[0])}, 'handler');
  assert.equal(view.partial, true);
  assert.equal(view.edges.length, 120);
});

test('missing endpoints remain evidence, never fabricated source nodes', () => {
  const view = focus({nodes: nodes.slice(0, 1), edges}, 'handler');
  assert.equal(view.outgoing.length, 1);
  assert.equal(view.byId.has('helper'), false);
  assert.equal(view.groups.length, 1);
});

test('source grouping distinguishes equal symbol names in different files', () => {
  const view = focus({nodes, edges}, 'duplicate');
  assert.equal(view.selected.source_file, 'policy/check.go');
  assert.equal(view.groups.length, 3);
  assert.equal(view.groups.find(([file]) => file === 'auth/check.go')[1][0].id, 'helper');
});

test('memory nodes without source paths and empty graphs are supported', () => {
  const view = focus({nodes: [{id: 'memory:1', label: 'Decision'}], edges: []}, 'memory:1');
  assert.equal(view.selected.label, 'Decision');
  assert.equal(view.groups[0][0], 'No source path recorded');
  assert.equal(focus({nodes: [], edges: []}).selected, undefined);
});

test('an unavailable requested node falls back only within this response', () => {
  const view = focus({nodes, edges}, 'previous-project-node');
  assert.equal(view.selected.id, 'helper');
  assert.equal(view.byId.has('previous-project-node'), false);
});

test('nested routes follow handler registration in its recorded reverse direction', () => {
  const data = {nodes: [...nodes, {id: 'route', label: 'GET /users'}], edges: [
    {source: 'handler', target: 'route', relation: 'handles'}, ...edges,
  ]};
  const children = context.graphChildren(data, 'route');
  assert.equal(children.length, 1);
  assert.equal(children[0].node.id, 'handler');
  assert.equal(children[0].relation, 'handled by');
  assert.equal(context.graphChildren(data, 'handler').length, 1);
});

test('nested expansion retains ambiguous targets and counts parallel evidence', () => {
  const data = {nodes, edges: [...edges, edges[0], {source: 'handler', target: 'duplicate', relation: 'may_call', confidence: 'INFERRED'}]};
  const children = context.graphChildren(data, 'handler');
  assert.equal(children.length, 2);
  assert.equal(children[0].count, 2);
  assert.equal(children[1].confidence, 'INFERRED');
  assert.equal(children[1].relation, 'may call');
});

test('imports and containment are not presented as nested execution', () => {
  const data = {nodes, edges: ['contains', 'imports', 'references'].map(relation => ({source: 'handler', target: 'helper', relation}))};
  assert.equal(context.graphChildren(data, 'handler').length, 0);
});

test('mixed protocol branches preserve their relationship types', () => {
  for (const relation of ['http_calls', 'grpc_calls', 'event_delivers']) {
    const children = context.graphChildren({nodes, edges: [{source: 'handler', target: 'helper', relation}]}, 'handler');
    assert.equal(children.length, 1);
    assert.equal(children[0].relation, relation.replaceAll('_', ' '));
  }
});
