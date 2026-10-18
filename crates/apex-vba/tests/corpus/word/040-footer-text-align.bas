' corpus-kind: word
' corpus-source: generated
' corpus-expect: ok
' corpus-note: ParagraphFormat on a header/footer Range is not supported
' corpus-flagged: no
' corpus-prints: Report 2024
' corpus-prints: 1
Sub Main()
    With ActiveDocument.Sections(1).Footers(wdHeaderFooterPrimary).Range
        .Text = "Report 2024"
        .ParagraphFormat.Alignment = wdAlignParagraphCenter
    End With
    Debug.Print ActiveDocument.Sections(1).Footers(wdHeaderFooterPrimary).Range.Text
    Debug.Print ActiveDocument.Sections(1).Footers(wdHeaderFooterPrimary).Range.ParagraphFormat.Alignment
End Sub
