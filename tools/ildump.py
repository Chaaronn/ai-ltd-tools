import sys, dnfile
from dncil.cil.body import CilMethodBody
from dncil.cil.body.reader import CilMethodBodyReaderBase
from dncil.clr.token import Token, StringToken, InvalidToken
from dncil.cil.opcode import OpCodes
import os
DLL=os.environ.get("GAME_DLL", r"C:\Program Files (x86)\Steam\steamapps\common\AI Learns To Race\data_AILearnsToDrive_windows_x86_64\AILearnsToDrive.dll")
pe=dnfile.dnPE(DLL)
class R(CilMethodBodyReaderBase):
    def __init__(s, pe, row): s.pe=pe; s.off=pe.get_offset_from_rva(row.Rva)
    def read(s, n): d=s.pe.get_data(s.pe.get_rva_from_offset(s.off), n); s.off+=n; return d
    def tell(s): return s.off
    def seek(s, o): s.off=o; return o
def resolve(tok):
    if isinstance(tok, StringToken):
        try: return repr(pe.net.user_strings.get_us(tok.rid).value)
        except Exception: return '<str>'
    if isinstance(tok, Token):
        t=tok.table; rid=tok.rid
        try:
            tbl={0x0a:'MemberRef',0x06:'MethodDef',0x04:'Field',0x01:'TypeRef',0x02:'TypeDef',0x2b:'MethodSpec',0x1b:'TypeSpec'}.get(t)
            if tbl=='MemberRef':
                r=pe.net.mdtables.MemberRef[rid-1]; p=r.Class.row
                pn=getattr(p,'TypeName',None) or ''
                return f"{pn}::{r.Name}"
            if tbl=='MethodDef': return "M:"+str(pe.net.mdtables.MethodDef[rid-1].Name)
            if tbl=='Field': return "F:"+str(pe.net.mdtables.Field[rid-1].Name)
            if tbl in('TypeRef','TypeDef'): return "T:"+str(getattr(pe.net.mdtables, tbl)[rid-1].TypeName)
        except Exception as e: return f"<{tbl} {rid}>"
    return str(tok)
def types(pattern):
    td=pe.net.mdtables.TypeDef
    for i,t in enumerate(td):
        full=f"{t.TypeNamespace}.{t.TypeName}"
        if pattern in full: yield t, full
def dump(pattern, methods=True, only=None):
    for t, full in types(pattern):
        print("=== TYPE", full, "extends", getattr(t.Extends.row,'TypeName',None) if t.Extends and t.Extends.row else None)
        for f in t.FieldList: print("  field", f.row.Name)
        if not methods: continue
        for m in t.MethodList:
            row=m.row
            if only and not any(o in str(row.Name) for o in only): continue
            print("  -- method", row.Name)
            if not row.Rva: continue
            try:
                body=CilMethodBody(R(pe,row))
                for ins in body.instructions:
                    op=ins.opcode.name; arg=ins.operand
                    a=resolve(arg) if isinstance(arg,(Token,)) else (arg if arg is not None else '')
                    print(f"     {op} {a}")
            except Exception as e: print("     <err>", e)
if __name__=='__main__':
    dump(sys.argv[1], methods=len(sys.argv)<3 or sys.argv[2]!='nometh', only=sys.argv[3:] or None)
