const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const tasks = [
  { id: 'one', itemKey: { itemId: 'one' }, command: 'first command' },
  { id: 'two', itemKey: { itemId: 'two' }, command: 'second command' },
];
let output = 'first line';
let state = [false, null];
let stateIndex = 0;
const jsx = (type, props, key) => ({ type, props, key });
const context = {
  o: { jsx, jsxs: jsx },
  c: {
    gh: () => ({ value: { routeKind: 'local-thread', conversationId: 'thread' } }),
    _U: (selector, key) => selector === 'tasks' ? tasks : key.itemId === 'one' ? output : 'second output',
  },
  eX: { GD: 'scope' },
  f: {
    lazy: (loader) => loader,
    useState: (initial) => {
      const index = stateIndex++;
      if (state[index] === undefined) state[index] = initial;
      return [state[index], (value) => { state[index] = value; }];
    },
  },
  n: Object.assign(() => ({ T: 'tasks', w: 'output' }), { e: async () => {} }),
};
vm.createContext(context);
vm.runInContext(fs.readFileSync(require('node:path').join(__dirname, 'background-panel.js'), 'utf8'), context);
(async () => {
  const { default: Panel } = await context.xkBackgroundPanel();
  const render = () => { stateIndex = 0; return Panel(); };
  let view = render();
  assert.equal(view.props.children[0].props.children[1], 2);
  view.props.children[0].props.onClick();
  view = render();
  const panel = view.props.children[1];
  assert.equal(panel.props.role, 'region');
  assert.equal(panel.props.style.resize, 'both');
  assert.equal(panel.props.style.width, 'min(360px, 75vw)');
  assert.equal(panel.props.style.height, 'min(320px, 55vh)');
  const outputNode = panel.props.children[2].props.children[0];
  assert.equal(outputNode.type(outputNode.props).props.children, 'first line');
  output = 'first line\nsecond line';
  assert.equal(outputNode.type(outputNode.props).props.children, output);
  panel.props.children[1].props.children[1].props.onClick();
  view = render();
  const changedOutput = view.props.children[1].props.children[2].props.children[0];
  assert.equal(changedOutput.type(changedOutput.props).props.children, 'second output');
  const closeButton = view.props.children[1].props.children[0].props.children[3];
  assert.equal(closeButton.props['aria-label'], '关闭后台任务面板');
  closeButton.props.onClick();
  assert.equal(render().props.children[1], null);
  tasks.splice(0);
  assert.equal(render(), null);
  console.log('PANEL_BEHAVIOR_OK');
})().catch((err) => { console.error(err); process.exitCode = 1; });
