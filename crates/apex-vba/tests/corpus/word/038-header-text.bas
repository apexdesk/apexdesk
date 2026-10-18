' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Confidential
Sub Main()
    With ActiveDocument.Sections(1).Headers(wdHeaderFooterPrimary)
        .Range.Text = "Confidential"
    End With
    Debug.Print ActiveDocument.Sections(1).Headers(wdHeaderFooterPrimary).Range.Text
End Sub
