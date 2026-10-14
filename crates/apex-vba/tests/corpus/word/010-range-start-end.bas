' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: quick
Sub Main()
    Dim r As Range
    Set r = ActiveDocument.Range(Start:=4, End:=9)
    Debug.Print r.Text
End Sub
