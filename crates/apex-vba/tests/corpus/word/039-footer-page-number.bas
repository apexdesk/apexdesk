' corpus-kind: word
' corpus-source: generated
' corpus-expect: ok
' corpus-note: collapsing a footer Range / Footers.PageNumbers is not supported
' corpus-flagged: no
' corpus-prints: 1
Sub Main()
    With ActiveDocument.Sections(1).Footers(wdHeaderFooterPrimary)
        .Range.Text = "Page "
        .Range.Collapse wdCollapseEnd
        .PageNumbers.Add
    End With
    Debug.Print ActiveDocument.Sections.Count
End Sub
