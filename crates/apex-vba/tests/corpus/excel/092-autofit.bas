' corpus-source: generated
' corpus-flagged: no
' corpus-prints: ok
Sub Main()
    Range("A1").Value = "A fairly long piece of text"
    Columns(1).AutoFit
    Rows(1).AutoFit
    Debug.Print "ok"
End Sub
