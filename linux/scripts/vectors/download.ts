import { downloadName } from '../../src/core/downloadname.ts'

/** The name a download is saved under, from the server's disposition, the page's attribute and the address. */
export function downloadNames(): { disposition: string | null, attribute: string, path: string, name: string }[] {
  const cases: [string | null, string, string][] = [
    ['attachment; filename="mnema-talk-export.zip"', '', '/api/users/me/export/download'],
    ["attachment; filename*=UTF-8''r%C3%A9sum%C3%A9.pdf; filename=\"resume.pdf\"", '', '/api/media/x'],
    ['attachment; filename=plain.txt', '', '/api/media/x'],
    ['attachment', 'notes.txt', '/api/media/x'],
    [null, '', '/api/media/abc/file.png'],
    ['attachment; filename="../../etc/passwd"', '', '/api/media/x'],
    ['attachment; filename="..\\evil.exe"', '', '/api/media/x'],
    ['attachment; filename=".hidden"', '', '/api/media/x'],
    ['attachment; filename="a\u0007b.txt"', '', '/api/media/x'],
    [null, '', '/api/'],
    // Candidates in order, and the ones that are skipped.
    ['attachment; filename=""', 'from-attribute.txt', '/api/media/x'],
    ['attachment; filename="  "', '', '/api/media/abc/from-path.png?download=1'],
    ["attachment; filename*=ISO-8859-1''caf%E9.txt; filename=\"cafe.txt\"", '', '/api/media/x'],
    ["attachment; filename*=UTF-8''bad%ZZ.txt; filename=\"fallback.txt\"", '', '/api/media/x'],
    ["attachment; filename*=UTF-8''%FF%FE.txt; filename=\"fallback.txt\"", '', '/api/media/x'],
    ['attachment; FILENAME="Upper.TXT"', '', '/api/media/x'],
    ['attachment; filename="semi;colon.txt"; size=3', '', '/api/media/x'],
    ['attachment; filename="a b.txt"', '', '/api/media/x'],
    ['attachment; filename=" ..  "', '', '/api/media/x'],
    ['attachment; filename="dir/"', '', '/api/media/x'],
    [null, '../up.txt', '/api/media/x'],
    [null, '', '/api/media/r%C3%A9sum%C3%A9.pdf'],
    [null, '', '/api/media/100%.txt'],
    // Names that must not leave the folder or mean something else on Windows.
    ['attachment; filename=".."', '', '/api/media/x'],
    ['attachment; filename="."', '', '/api/media/x'],
    ['attachment; filename="..\\..\\x"', '', '/api/media/x'],
    ['attachment; filename="C:evil.zip"', '', '/api/media/x'],
    ['attachment; filename="a.zip:stream"', '', '/api/media/x'],
    ['attachment; filename="<a>|b?*.txt"', '', '/api/media/x'],
    ["attachment; filename*=UTF-8''a%22b%3Ac%3E.txt; filename=\"plain.txt\"", '', '/api/media/x'],
    ['attachment; filename="NUL"', '', '/api/media/x'],
    ['attachment; filename="con.zip"', '', '/api/media/x'],
    ['attachment; filename="Lpt9.tar.gz"', '', '/api/media/x'],
    ['attachment; filename="COM10.txt"', '', '/api/media/x'],
    ['attachment; filename="aux .txt"', '', '/api/media/x'],
    ['attachment; filename="name. ."', '', '/api/media/x'],
    ['attachment; filename="tab\there\u0001.txt"', '', '/api/media/x'],
    ['attachment; filename="..hidden.txt"', '', '/api/media/x'],
    [null, '.profile', '/api/media/x'],
    [`attachment; filename="${'a'.repeat(300)}.zip"`, '', '/api/media/x'],
    [`attachment; filename="${'\u00e9'.repeat(150)}.pdf"`, '', '/api/media/x'],
    [`attachment; filename="${'b'.repeat(250)}"`, '', '/api/media/x'],
    [`attachment; filename="${'c'.repeat(195)}${'.'.repeat(1)}${'d'.repeat(30)}"`, '', '/api/media/x'],
  ]
  return cases.map(([disposition, attribute, path]) => ({ disposition, attribute, path, name: downloadName(disposition, attribute, path) }))
}
