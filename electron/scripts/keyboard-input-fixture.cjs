const assert = require('node:assert/strict');

exports.exerciseKeyboardInput = async (window) => {
  const namedKeys = { Backspace: 8, Enter: 13, ArrowLeft: 37 };
  const send = (type, key, extra = {}) => window.webContents.debugger.sendCommand('Input.dispatchKeyEvent', {
    type, key, code: namedKeys[key] ? key : /[0-9]/.test(key) ? `Digit${key}` : `Key${key.toUpperCase()}`,
    windowsVirtualKeyCode: namedKeys[key] ?? key.toUpperCase().charCodeAt(0),
    ...(type === 'keyDown' && key.length === 1 ? { text: key, unmodifiedText: key } : {}), ...extra,
  });
  const press = async (key, duplicate = false) => {
    await send('keyDown', key);
    if (duplicate) await send('keyDown', key);
    await send('keyUp', key);
  };
  const setup = mode => window.webContents.executeJavaScript(`window.keyboardInputFixture.setup('${mode}')`);
  const read = () => window.webContents.executeJavaScript('window.keyboardInputFixture.read()');
  const reports = [];
  for (const mode of ['text', 'terminal']) {
    await setup(mode);
    // Wait for the hidden window's first rendered surface. CDP otherwise
    // acknowledges early key events without delivering them to the document.
    await window.webContents.capturePage();
    for (const key of 'abc123') await press(key, true);
    assert.equal(await read(), 'aabbcc112233', `${mode}: negative control did not reproduce doubling`);
    await press('Backspace', true);
    assert.equal(await read(), mode === 'text' ? 'aabbcc1122' : 'aabbcc112233\x7f\x7f',
      `${mode}: negative control did not reproduce doubled backspace`);
    // Replay the captured DOM sequence through the production event guard.
    await window.webContents.executeJavaScript('window.keyboardInputFixture.guard(true)');
    try {
      await setup(mode);
      for (const key of 'abc123') await press(key, true);
      assert.equal(await read(), 'abc123', `${mode}: duplicated physical downs escaped the guard`);
      for (const key of 'bookkeeper') await press(key);
      assert.equal(await read(), 'abc123bookkeeper', `${mode}: intentional repeated letters were lost`);
      await send('keyDown', 'x');
      for (let i = 0; i < 3; i++) await send('keyDown', 'x', { autoRepeat: true });
      await send('keyUp', 'x');
      assert.equal(await read(), 'abc123bookkeeperxxxx', `${mode}: held-key repeat was lost`);
      await send('keyDown', 'a');
      await window.webContents.executeJavaScript('window.dispatchEvent(new Event("blur"))');
      await send('keyDown', 'a');
      await send('keyUp', 'a');
      assert.equal(await read(), 'abc123bookkeeperxxxxaa', `${mode}: focus loss left a stale held key`);
      await setup(mode);
      for (const key of 'abcdefghij') await press(key);
      await press('Backspace', true);
      assert.equal(await read(), mode === 'text' ? 'abcdefghi' : 'abcdefghij\x7f',
        `${mode}: one backspace deleted twice`);
      await press('Backspace');
      await press('Backspace');
      assert.equal(await read(), mode === 'text' ? 'abcdefg' : 'abcdefghij' + '\x7f'.repeat(3),
        `${mode}: deliberate consecutive backspaces were lost`);
      await send('keyDown', 'Backspace');
      for (let i = 0; i < 2; i++) await send('keyDown', 'Backspace', { autoRepeat: true });
      await send('keyUp', 'Backspace');
      assert.equal(await read(), mode === 'text' ? 'abcd' : 'abcdefghij' + '\x7f'.repeat(6),
        `${mode}: held-backspace repeat was lost`);
      if (mode === 'terminal') {
        await setup(mode);
        await press('ArrowLeft', true);
        await press('Enter', true);
        assert.equal(await read(), '\x1b[D\r', 'terminal: navigation or Enter was sent twice');
      }
      reports.push({ mode, negativeControl: 'aabbcc112233', guarded: 'abc123',
        repeatedLetters: 'bookkeeper', heldRepeat: 4, focusRecovery: 'aa',
        backspace: 'single, consecutive and held presses passed', navigationAndEnter: mode === 'terminal' });
    } finally { await window.webContents.executeJavaScript('window.keyboardInputFixture.guard(false)'); }
    await setup(mode);
    await press('a', true);
    assert.equal(await read(), 'aa', `${mode}: disposed guard still intercepted input`);
    if (mode === 'terminal') {
      // Ctrl+F is the program's (cursor forward, page down); only ⌘F opens
      // the pane's search box. CDP modifiers: Ctrl 2, Meta 4.
      const chord = async (modifiers) => {
        for (const type of ['rawKeyDown', 'keyUp']) {
          await window.webContents.debugger.sendCommand('Input.dispatchKeyEvent', {
            type, key: 'f', code: 'KeyF', windowsVirtualKeyCode: 70, modifiers,
          });
        }
      };
      const fixture = 'window.keyboardInputFixture';
      await setup(mode);
      await chord(2);
      assert.equal(await window.webContents.executeJavaScript(`${fixture}.sent()`), '\x06',
        'terminal: Ctrl+F did not reach the program');
      assert.equal(await window.webContents.executeJavaScript(`${fixture}.searchOpen()`), false,
        'terminal: Ctrl+F opened the search box');
      await chord(4);
      assert.equal(await window.webContents.executeJavaScript(`${fixture}.searchOpen()`), true,
        'terminal: ⌘F did not open the search box');
      assert.equal(await window.webContents.executeJavaScript(`${fixture}.sent()`), '\x06',
        'terminal: ⌘F reached the program');
      reports.at(-1).ctrlF = 'program';
    }
  }
  return reports;
};
