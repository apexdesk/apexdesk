' corpus-kind: word
' corpus-source: generated
' corpus-expect: ok
' corpus-note: Fields.Add is not supported
' corpus-flagged: no
' corpus-prints: 1
Sub Main()
    Dim r As Range
    Set r = ActiveDocument.Paragraphs(3).Range
    r.InsertAfter " Page "
    r.Collapse wdCollapseEnd
    ActiveDocument.Fields.Add r, wdFieldPage
    Debug.Print ActiveDocument.Fields.Count
End Sub
