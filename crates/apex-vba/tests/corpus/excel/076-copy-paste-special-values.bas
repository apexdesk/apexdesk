' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 6
' corpus-prints: 6
Sub Main()
    Range("A1").Value = 2
    Range("A2").Value = 3
    Range("A3").Formula = "=A1*A2"
    Range("A3").Copy
    Range("B3").PasteSpecial Paste:=xlPasteValues
    Application.CutCopyMode = False
    Debug.Print Range("B3").Value
    Debug.Print Range("A3").Value
End Sub
