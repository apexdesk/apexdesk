' corpus-source: generated
' corpus-flagged: no
' corpus-prints: -1
' corpus-prints: 0
Sub Main()
    Worksheets.Add.Name = "Hidden"
    Sheets("Hidden").Visible = xlSheetHidden
    Rows(2).Hidden = True
    Columns(3).Hidden = True
    Debug.Print IIf(Rows(2).Hidden, -1, 0)
    Rows(2).Hidden = False
    Debug.Print IIf(Rows(2).Hidden, -1, 0)
End Sub
