' corpus-kind: word
' corpus-source: generated
' corpus-expect: ok
' corpus-note: InsertBreak wdPageBreak does not put a page-break character (Chr(12)) in Range.Text
' corpus-flagged: no
' corpus-prints: True
Sub Main()
    Dim r As Range
    Set r = ActiveDocument.Paragraphs(2).Range
    r.Collapse wdCollapseEnd
    r.InsertBreak wdPageBreak
    Debug.Print InStr(ActiveDocument.Content.Text, Chr(12)) > 0
End Sub
