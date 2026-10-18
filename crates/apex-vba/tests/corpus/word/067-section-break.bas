' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 2
Sub Main()
    Dim r As Range
    Set r = ActiveDocument.Paragraphs(2).Range
    r.Collapse wdCollapseEnd
    r.InsertBreak wdSectionBreakNextPage
    Debug.Print ActiveDocument.Sections.Count
End Sub
