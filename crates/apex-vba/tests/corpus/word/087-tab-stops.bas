' corpus-kind: word
' corpus-source: generated
' corpus-expect: ok
' corpus-note: Paragraph.TabStops is not supported
' corpus-flagged: no
' corpus-prints: done
Sub Main()
    With ActiveDocument.Paragraphs(1)
        .TabStops.ClearAll
        .TabStops.Add Position:=InchesToPoints(2), Alignment:=wdAlignTabLeft
    End With
    Debug.Print "done"
End Sub
