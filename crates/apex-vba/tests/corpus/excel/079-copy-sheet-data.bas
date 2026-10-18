' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 42
' corpus-prints: Backup
Sub Main()
    Dim ws2 As Worksheet
    Range("A1").Value = 42
    Set ws2 = Worksheets.Add(After:=Sheets(Sheets.Count))
    ws2.Name = "Backup"
    Sheets("Sheet1").Range("A1:A1").Copy ws2.Range("A1")
    Debug.Print ws2.Range("A1").Value
    Debug.Print ws2.Name
End Sub
