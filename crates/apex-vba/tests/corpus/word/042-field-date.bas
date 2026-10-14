' corpus-kind: word
' corpus-source: generated
' corpus-expect: ok
' corpus-note: Fields.Add is not supported
' corpus-flagged: no
' corpus-prints: 1
Sub Main()
    Dim r As Range
    Set r = ActiveDocument.Range(0, 0)
    ActiveDocument.Fields.Add Range:=r, Type:=wdFieldDate
    Debug.Print ActiveDocument.Fields.Count
End Sub
