' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Pass
' corpus-prints: Fail
Sub Main()
    Range("A1").Value = 80: Range("A2").Value = 30
    Range("B1:B2").Formula = "=IF(A1>=50,""Pass"",""Fail"")"
    Debug.Print Range("B1").Value
    Debug.Print Range("B2").Value
End Sub
